//! Real-time TD-PSOLA pitch shifter with independent formant control.
//!
//! Analysis marks are laid one period apart (snapped to waveform peaks while
//! voiced). Each synthesis voice places Hann grains, two analysis periods
//! wide, at its own spacing of `period / ratio`; reading the grain at a
//! stretched rate shifts formants independently of pitch. Because grains are
//! pitch-synchronous the spectral envelope is preserved, which is what keeps
//! a corrected voice sounding like the singer instead of a chipmunk.
//!
//! Latency is a fixed `delay` samples. Grains are scheduled just in time, so
//! low voices whose periods do not fit the delay borrow the newest complete
//! period instead of stalling.

pub const MAX_VOICES: usize = 5;
const BUF: usize = 1 << 14;
const MASK: usize = BUF - 1;
const MAX_MARKS: usize = 64;
const WIN_LEN: usize = 1024;

#[derive(Clone, Copy, Default)]
struct Mark {
    pos: u64,
    period: f32,
    voiced: bool,
}

#[derive(Clone, Copy)]
pub struct VoiceTarget {
    pub active: bool,
    pub ratio: f32,
    pub formant: f32,
    pub gain: f32,
}

impl Default for VoiceTarget {
    fn default() -> Self {
        Self { active: false, ratio: 1.0, formant: 1.0, gain: 0.0 }
    }
}

struct Voice {
    next_synth: f64,
    acc: Vec<f32>,
    wsum: Vec<f32>,
    gain: f32,
    target: VoiceTarget,
}

impl Voice {
    fn new() -> Self {
        Self {
            next_synth: 0.0,
            acc: vec![0.0; BUF],
            wsum: vec![0.0; BUF],
            gain: 0.0,
            target: VoiceTarget::default(),
        }
    }
}

pub struct Psola {
    input: Vec<f32>,
    n_in: u64,
    marks: [Mark; MAX_MARKS],
    mark_head: usize,
    mark_count: usize,
    next_mark: f64,
    period: f32,
    voiced: bool,
    unvoiced_period: f32,
    max_period: f32,
    delay: u64,
    voices: Vec<Voice>,
    window: Vec<f32>,
    gain_coef: f32,
}

impl Psola {
    pub fn new(fs: f32) -> Self {
        let window = (0..=WIN_LEN)
            .map(|i| 0.5 + 0.5 * (std::f32::consts::PI * i as f32 / WIN_LEN as f32).cos())
            .collect();
        let unvoiced_period = (fs * 0.006).round();
        Self {
            input: vec![0.0; BUF],
            n_in: 0,
            marks: [Mark::default(); MAX_MARKS],
            mark_head: 0,
            mark_count: 0,
            next_mark: 0.0,
            period: unvoiced_period,
            voiced: false,
            unvoiced_period,
            max_period: fs / super::pitch::MIN_FREQ,
            delay: (fs * 0.010).round() as u64,
            voices: (0..MAX_VOICES).map(|_| Voice::new()).collect(),
            window,
            gain_coef: 1.0 - (-1.0 / (0.01 * fs)).exp(),
        }
    }

    pub fn latency(&self) -> usize {
        self.delay as usize
    }

    /// Update the analysis period. `period` is in samples; pass `None` when unvoiced.
    pub fn set_period(&mut self, period: Option<f32>) {
        match period {
            Some(p) if p > 8.0 => {
                self.period = p.min(self.max_period);
                self.voiced = true;
            }
            _ => {
                self.period = self.unvoiced_period;
                self.voiced = false;
            }
        }
    }

    pub fn set_voice(&mut self, idx: usize, target: VoiceTarget) {
        if let Some(v) = self.voices.get_mut(idx) {
            v.target = target;
        }
    }

    /// The input sample that lines up with the current output, for dry mixing.
    #[inline]
    pub fn dry(&self) -> f32 {
        let u = self.n_in as i64 - self.delay as i64 - 1;
        if u < 0 {
            0.0
        } else {
            self.input[u as usize & MASK]
        }
    }

    /// Push one input sample, then write each voice's output for the
    /// corresponding (delayed) instant into `out`.
    #[inline]
    pub fn process(&mut self, x: f32, out: &mut [f32; MAX_VOICES]) {
        self.input[self.n_in as usize & MASK] = x;
        self.n_in += 1;
        self.place_marks();

        let u = self.n_in as i64 - self.delay as i64 - 1;
        if u < 0 {
            *out = [0.0; MAX_VOICES];
            return;
        }
        let u = u as u64;
        for vi in 0..MAX_VOICES {
            let target_gain = if self.voices[vi].target.active { self.voices[vi].target.gain } else { 0.0 };
            let v = &mut self.voices[vi];
            v.gain += (target_gain - v.gain) * self.gain_coef;
            if !v.target.active && v.gain < 1e-4 {
                // Keep the synthesis clock aligned so the voice re-enters cleanly.
                v.next_synth = u as f64;
                let i = u as usize & MASK;
                v.acc[i] = 0.0;
                v.wsum[i] = 0.0;
                out[vi] = 0.0;
                continue;
            }
            self.place_grains(vi, u);
            let v = &mut self.voices[vi];
            let i = u as usize & MASK;
            let y = v.acc[i] / v.wsum[i].max(1.0);
            v.acc[i] = 0.0;
            v.wsum[i] = 0.0;
            out[vi] = y * v.gain;
        }
    }

    fn place_marks(&mut self) {
        let n = self.n_in as f64;
        let p = self.period as f64;
        // Resynchronise after silence or a reset rather than racing to catch up.
        if self.next_mark < n - 4.0 * self.max_period as f64 {
            self.next_mark = n - 2.0 * p;
        }
        loop {
            let search = if self.voiced { (p * 0.25).floor() } else { 0.0 };
            let cand = self.next_mark;
            if cand + search + p >= n {
                break;
            }
            let mut pos = cand.round().max(0.0) as u64;
            if self.voiced && search >= 1.0 {
                let lo = (cand - search).max(0.0) as u64;
                let hi = (cand + search) as u64;
                let mut best = f32::MIN;
                for i in lo..=hi {
                    let v = self.input[i as usize & MASK];
                    if v > best {
                        best = v;
                        pos = i;
                    }
                }
            }
            if let Some(last) = self.last_mark() {
                // Never let refinement collapse marks together.
                let min_pos = last.pos + (p * 0.5) as u64;
                pos = pos.max(min_pos);
            }
            self.push_mark(Mark { pos, period: self.period, voiced: self.voiced });
            self.next_mark = pos as f64 + p;
        }
    }

    fn last_mark(&self) -> Option<Mark> {
        if self.mark_count == 0 {
            None
        } else {
            Some(self.marks[(self.mark_head + MAX_MARKS - 1) % MAX_MARKS])
        }
    }

    fn push_mark(&mut self, m: Mark) {
        self.marks[self.mark_head] = m;
        self.mark_head = (self.mark_head + 1) % MAX_MARKS;
        self.mark_count = (self.mark_count + 1).min(MAX_MARKS);
    }

    /// Complete mark (whole grain available) closest to `s`.
    fn nearest_mark(&self, s: f64) -> Option<Mark> {
        let mut best: Option<Mark> = None;
        let mut best_d = f64::MAX;
        for k in 0..self.mark_count {
            let m = self.marks[(self.mark_head + MAX_MARKS - 1 - k) % MAX_MARKS];
            let reach = m.pos as f64 + m.period as f64 * 2.0;
            if reach > self.n_in as f64 {
                continue;
            }
            let d = (m.pos as f64 - s).abs();
            if d < best_d {
                best_d = d;
                best = Some(m);
            } else if (m.pos as f64) < s {
                break;
            }
        }
        best
    }

    fn place_grains(&mut self, vi: usize, u: u64) {
        let uf = u as f64;
        if self.voices[vi].next_synth < uf - self.max_period as f64 * 2.0 {
            self.voices[vi].next_synth = uf;
        }
        loop {
            let s = self.voices[vi].next_synth;
            let Some(mark) = self.nearest_mark(s) else {
                self.voices[vi].next_synth = uf + 1.0;
                return;
            };
            let target = self.voices[vi].target;
            let phi = target.formant.clamp(0.5, 2.0) as f64;
            let h = mark.period as f64 / phi;
            if s - h > uf {
                return;
            }
            self.add_grain(vi, s, h, phi, mark, u);
            let ratio = if mark.voiced { target.ratio.clamp(0.25, 4.0) as f64 } else { 1.0 };
            self.voices[vi].next_synth = s + mark.period as f64 / ratio;
        }
    }

    fn add_grain(&mut self, vi: usize, s: f64, h: f64, phi: f64, mark: Mark, u: u64) {
        let j0 = (s - h).ceil().max(u as f64) as u64;
        let j1 = (s + h).floor() as u64;
        let a = mark.pos as f64;
        let wscale = WIN_LEN as f64 / h;
        let oldest = self.n_in.saturating_sub(BUF as u64 - 2) as f64;
        let newest = (self.n_in - 1) as f64;
        let input = &self.input;
        let window = &self.window;
        let v = &mut self.voices[vi];
        for j in j0..=j1 {
            let t = j as f64 - s;
            let wi = (t.abs() * wscale) as usize;
            if wi > WIN_LEN {
                continue;
            }
            let w = window[wi];
            let src = (a + t * phi).clamp(oldest, newest);
            let i0 = src.floor();
            let frac = (src - i0) as f32;
            let i0 = i0 as usize;
            let x0 = input[i0 & MASK];
            let x1 = input[(i0 + 1) & MASK];
            let x = x0 + (x1 - x0) * frac;
            let k = j as usize & MASK;
            v.acc[k] += w * x;
            v.wsum[k] += w;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::pitch::PitchDetector;

    fn run(freq: f32, ratio: f32) -> f32 {
        let fs = 48_000.0;
        let mut ps = Psola::new(fs);
        ps.set_voice(0, VoiceTarget { active: true, ratio, formant: 1.0, gain: 1.0 });
        let mut det_in = PitchDetector::new(fs);
        let mut det_out = PitchDetector::new(fs);
        let mut out = [0.0; MAX_VOICES];
        let mut last = 0.0;
        for i in 0..(fs as usize) {
            let t = i as f32 / fs;
            let x = (1..6)
                .map(|h| (2.0 * std::f32::consts::PI * freq * h as f32 * t).sin() / h as f32)
                .sum::<f32>()
                * 0.3;
            if let Some(e) = det_in.push(x) {
                ps.set_period((e.clarity > 0.8).then(|| fs / e.freq));
            }
            ps.process(x, &mut out);
            if let Some(e) = det_out.push(out[0]) {
                if e.clarity > 0.8 {
                    last = e.freq;
                }
            }
        }
        last
    }

    #[test]
    fn shifts_pitch_by_ratio() {
        for &(f, semis) in &[(220.0f32, 3.0f32), (220.0, -4.0), (140.0, 2.0), (330.0, -1.0), (440.0, 0.0)] {
            let ratio = 2f32.powf(semis / 12.0);
            let got = run(f, ratio);
            let cents = 1200.0 * (got / (f * ratio)).log2();
            assert!(cents.abs() < 12.0, "f={f} semis={semis} got {got} ({cents} cents)");
        }
    }
}
