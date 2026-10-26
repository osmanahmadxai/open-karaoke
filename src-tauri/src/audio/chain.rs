//! The complete microphone path: clean-up → pitch tracking → autotune /
//! harmonies / doubling (PSOLA) → channel strip → time-based effects.

use super::fx::*;
use super::params::Params;
use super::pitch::{PitchDetector, PitchEstimate};
use super::psola::{Psola, VoiceTarget, MAX_VOICES};
use super::scale;

#[derive(Clone, Copy, Default, Debug)]
pub struct PitchFrame {
    pub voiced: bool,
    /// Sung pitch as fractional MIDI.
    pub input: f32,
    /// Pitch after correction as fractional MIDI.
    pub output: f32,
    /// Scale note being targeted.
    pub target: i32,
    pub clarity: f32,
    /// Input level in dBFS.
    pub level_db: f32,
}

const LEAD: usize = 0;
const HARM_A: usize = 1;
const HARM_B: usize = 2;
const DBL_L: usize = 3;
const DBL_R: usize = 4;

struct Controller {
    note: Option<i32>,
    held: f32,
    correction: f32,
    vib_phase: f32,
    vib_amt: f32,
    history: [f32; 3],
    streak: u32,
}

impl Controller {
    fn new() -> Self {
        Self { note: None, held: 0.0, correction: 0.0, vib_phase: 0.0, vib_amt: 0.0, history: [0.0; 3], streak: 0 }
    }

    /// Median of the last three voiced estimates rejects one-frame octave slips.
    fn smooth(&mut self, m: f32) -> f32 {
        self.history = [self.history[1], self.history[2], m];
        self.streak += 1;
        if self.streak < 3 {
            return m;
        }
        let mut h = self.history;
        h.sort_by(|a, b| a.partial_cmp(b).unwrap());
        h[1]
    }
}

struct DelayLine {
    buf: Vec<f32>,
    pos: usize,
    len: usize,
}

impl DelayLine {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len + 1], pos: 0, len }
    }
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.buf[self.pos];
        self.buf[self.pos] = x;
        self.pos = (self.pos + 1) % (self.len + 1);
        y
    }
}

pub struct VocalChain {
    fs: f32,
    hpf: SmartBiquad,
    gate: Gate,
    detector: PitchDetector,
    psola: Psola,
    ctl: Controller,
    lead_ratio: f32,
    eq: [[SmartBiquad; 3]; 2],
    deess: [DeEsser; 2],
    comp: Compressor,
    delay: Delay,
    reverb: Reverb,
    dbl_delay: [DelayLine; 2],
    voices: [f32; MAX_VOICES],
    level_env: f32,
}

impl VocalChain {
    pub fn new(fs: f32) -> Self {
        Self {
            fs,
            hpf: SmartBiquad::default(),
            gate: Gate::new(fs),
            detector: PitchDetector::new(fs),
            psola: Psola::new(fs),
            ctl: Controller::new(),
            lead_ratio: 1.0,
            eq: Default::default(),
            deess: [DeEsser::new(fs), DeEsser::new(fs)],
            comp: Compressor::new(fs),
            delay: Delay::new(fs),
            reverb: Reverb::new(fs),
            dbl_delay: [DelayLine::new((fs * 0.013) as usize), DelayLine::new((fs * 0.021) as usize)],
            voices: [0.0; MAX_VOICES],
            level_env: 0.0,
        }
    }

    /// Processing latency of the chain in samples.
    pub fn latency(&self) -> usize {
        self.psola.latency()
    }

    /// Process a block of mono microphone samples into interleaved stereo.
    /// `on_frame` receives one pitch frame per analysis hop.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>, p: &Params, mut on_frame: impl FnMut(PitchFrame)) {
        let fs = self.fs;
        self.hpf.update(Shape::HighPass, p.hpf_hz.get().clamp(20.0, 400.0), 0.707, fs);
        for ch in 0..2 {
            self.eq[ch][0].update(Shape::LowShelf(p.eq_low_db.get()), 220.0, 0.7, fs);
            self.eq[ch][1].update(Shape::Peak(p.eq_presence_db.get()), 3200.0, 0.9, fs);
            self.eq[ch][2].update(Shape::HighShelf(p.eq_air_db.get()), 10_000.0, 0.7, fs);
        }
        let gate_th = db_to_lin(p.gate_db.get());
        let gain = db_to_lin(p.vocal_gain_db.get());
        let mix = p.tune_mix.get().clamp(0.0, 1.0);
        let deess = p.deesser.get();
        let comp = p.comp.get();
        let (dl_mix, dl_ms, dl_fb) = (p.delay_mix.get(), p.delay_ms.get(), p.delay_feedback.get());
        let (rv_mix, rv_size, rv_damp) = (p.reverb_mix.get(), p.reverb_size.get(), p.reverb_damp.get());
        let spread = p.harmony_spread.get().clamp(0.0, 1.0);
        self.detector.cutoff = 0.86;

        out.clear();
        out.reserve(input.len() * 2);
        for &raw in input {
            let x = self.hpf.f.run(raw * gain);
            self.level_env += (x.abs() - self.level_env) * 0.002;
            let g = self.gate.run(x, gate_th);
            let x = x * g;

            if let Some(est) = self.detector.push(x) {
                let frame = self.control(est, p);
                on_frame(frame);
            }

            self.psola.process(x, &mut self.voices);
            let dry = self.psola.dry();
            let lead = dry * (1.0 - mix) + self.voices[LEAD] * mix;

            // Harmonies and doubles are panned around the centred lead.
            let pan = |v: f32, pos: f32| (v * (0.5 - 0.5 * pos).sqrt(), v * (0.5 + 0.5 * pos).sqrt());
            let (al, ar) = pan(self.voices[HARM_A], -spread * 0.6);
            let (bl, br) = pan(self.voices[HARM_B], spread * 0.6);
            let dbl_l = self.dbl_delay[0].run(self.voices[DBL_L]);
            let dbl_r = self.dbl_delay[1].run(self.voices[DBL_R]);
            let mut l = lead * 0.707 + al + bl + dbl_l * 0.8;
            let mut r = lead * 0.707 + ar + br + dbl_r * 0.8;

            for f in self.eq[0].iter_mut() {
                l = f.f.run(l);
            }
            for f in self.eq[1].iter_mut() {
                r = f.f.run(r);
            }
            l = self.deess[0].run(l, deess);
            r = self.deess[1].run(r, deess);
            let cg = self.comp.gain((l + r) * 0.5, comp);
            l *= cg;
            r *= cg;

            let (dl, dr) = self.delay.run(l, r, dl_ms, dl_fb);
            let (wl, wr) = (l + dl * dl_mix, r + dr * dl_mix);
            let (rl, rr) = self.reverb.run(wl, wr, rv_size, rv_damp);
            out.push(wl + rl * rv_mix * 2.0);
            out.push(wr + rr * rv_mix * 2.0);
        }
    }

    fn control(&mut self, est: PitchEstimate, p: &Params) -> PitchFrame {
        let dt = self.detector.hop() as f32 / self.fs;
        let level_db = lin_to_db(self.level_env);
        let voiced = est.freq > 0.0 && est.clarity >= p.tune_sensitivity.get().clamp(0.3, 0.99) && self.gate.is_open();

        let tonic = p.tune_key.get().round() as i32;
        let sc = scale::scale(p.tune_scale.get().round().max(0.0) as usize);
        let enabled = p.tune_enabled.get() > 0.5;
        let transpose = p.tune_transpose.get();
        let formant = 2f32.powf(p.tune_formant.get() / 12.0);
        let ctl = &mut self.ctl;

        if !voiced {
            ctl.streak = 0;
            ctl.note = None;
            ctl.held = 0.0;
            ctl.correction *= 0.85;
            ctl.vib_amt = 0.0;
            self.lead_ratio = 2f32.powf(transpose / 12.0);
            self.psola.set_period(None);
            self.psola.set_voice(LEAD, VoiceTarget { active: true, ratio: self.lead_ratio, formant, gain: 1.0 });
            for v in [HARM_A, HARM_B, DBL_L, DBL_R] {
                self.psola.set_voice(v, VoiceTarget { active: false, ratio: self.lead_ratio, formant, gain: 0.0 });
            }
            return PitchFrame { voiced: false, level_db, clarity: est.clarity, ..Default::default() };
        }

        self.psola.set_period(Some(self.fs / est.freq));
        let m = ctl.smooth(69.0 + 12.0 * (est.freq / 440.0).log2());

        // Choose the target note with hysteresis so wobbles near a boundary
        // don't make the output flip between notes.
        let nearest = scale::nearest(m, tonic, sc);
        let note = match ctl.note {
            Some(cur) if cur != nearest && scale::in_scale(cur, tonic, sc) && (m - cur as f32).abs() < (m - nearest as f32).abs() + 0.22 => cur,
            _ => nearest,
        };
        if ctl.note == Some(note) {
            ctl.held += dt;
        } else {
            ctl.held = 0.0;
        }
        ctl.note = Some(note);

        let mut desired = if enabled { note as f32 - m } else { 0.0 };
        let flex = p.tune_flex.get() / 100.0;
        if flex > 0.0 {
            // Leave small natural deviations alone, correct large ones fully.
            desired *= (desired.abs() / flex).powi(2).min(1.0);
        }
        desired *= p.tune_amount.get().clamp(0.0, 1.0);

        let sustain = (ctl.held / 0.35).min(1.0);
        let retune = p.tune_retune_ms.get().max(0.0) * 0.001 * (1.0 + 4.0 * p.tune_humanize.get() * sustain);
        if retune <= dt * 0.5 {
            ctl.correction = desired;
        } else {
            ctl.correction += (desired - ctl.correction) * (1.0 - (-dt / retune).exp());
        }

        // Synthetic vibrato fades in on held notes, like a trained singer.
        let depth = p.tune_vibrato_depth.get() / 100.0;
        let vib_target = if depth > 0.0 && ctl.held > 0.25 { depth } else { 0.0 };
        ctl.vib_amt += (vib_target - ctl.vib_amt) * (dt / 0.3).min(1.0);
        ctl.vib_phase = (ctl.vib_phase + dt * p.tune_vibrato_rate.get() * std::f32::consts::TAU) % std::f32::consts::TAU;
        let vibrato = ctl.vib_amt * ctl.vib_phase.sin();

        let shift = ctl.correction + vibrato + transpose;
        self.lead_ratio = 2f32.powf(shift / 12.0);
        self.psola.set_voice(LEAD, VoiceTarget { active: true, ratio: self.lead_ratio, formant, gain: 1.0 });

        let level = p.harmony_level.get().clamp(0.0, 1.0);
        for (voice, param) in [(HARM_A, &p.harmony_a), (HARM_B, &p.harmony_b)] {
            let degrees = param.get().round() as i32;
            if degrees == 0 || level <= 0.0 {
                self.psola.set_voice(voice, VoiceTarget { active: false, ..Default::default() });
                continue;
            }
            let harm = scale::shift_degrees(note, degrees, tonic, sc);
            let ratio = 2f32.powf((shift + (harm - note) as f32) / 12.0);
            // A touch of formant drop on lower harmonies keeps them from sounding thin.
            let hf = if harm < note { formant * 0.97 } else { formant };
            self.psola.set_voice(voice, VoiceTarget { active: true, ratio, formant: hf, gain: level * 0.8 });
        }

        let dbl = p.doubler.get().clamp(0.0, 1.0);
        for (voice, cents) in [(DBL_L, -9.0f32), (DBL_R, 11.0)] {
            self.psola.set_voice(
                voice,
                VoiceTarget {
                    active: dbl > 0.0,
                    ratio: self.lead_ratio * 2f32.powf(cents / 1200.0),
                    formant,
                    gain: dbl * 0.7,
                },
            );
        }

        PitchFrame { voiced: true, input: m, output: m + shift, target: note, clarity: est.clarity, level_db }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_tune_pulls_flat_note_to_scale() {
        let fs = 48_000.0;
        let p = Params::default();
        p.tune_retune_ms.set(0.0);
        p.tune_humanize.set(0.0);
        p.reverb_mix.set(0.0);
        p.gate_db.set(-80.0);
        let mut chain = VocalChain::new(fs);
        // A4 sung 40 cents flat in C major → should land on A4 (69).
        let f = 440.0 * 2f32.powf(-0.4 / 12.0);
        let input: Vec<f32> = (0..fs as usize)
            .map(|i| {
                let t = i as f32 / fs;
                (1..6).map(|h| (std::f32::consts::TAU * f * h as f32 * t).sin() / h as f32).sum::<f32>() * 0.2
            })
            .collect();
        let mut out = Vec::new();
        let mut frames = Vec::new();
        chain.process(&input, &mut out, &p, |fr| frames.push(fr));
        let last = frames.iter().rev().find(|f| f.voiced).expect("voiced frames");
        assert_eq!(last.target, 69);
        assert!((last.output - 69.0).abs() < 0.05, "{last:?}");

        // Verify the actual audio, not just the controller's intent.
        let mut det = PitchDetector::new(fs);
        let mut got = 0.0;
        for s in out.chunks(2).skip(fs as usize / 2) {
            if let Some(e) = det.push(s[0]) {
                if e.clarity > 0.8 {
                    got = e.freq;
                }
            }
        }
        let cents = 1200.0 * (got / 440.0).log2();
        assert!(cents.abs() < 12.0, "output {got} Hz ({cents} cents from A4)");
    }
}
