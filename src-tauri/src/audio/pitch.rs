//! McLeod Pitch Method (MPM) detector.
//!
//! The normalised square difference function is computed with an FFT-based
//! autocorrelation, so a full analysis costs two real FFTs of 2×window.
//! Analysis runs every `hop` samples over the most recent `window` samples.

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::Arc;

pub const MIN_FREQ: f32 = 65.0;
pub const MAX_FREQ: f32 = 1200.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct PitchEstimate {
    /// Fundamental in Hz, 0 when unvoiced.
    pub freq: f32,
    /// NSDF peak height, 0..1. Higher is more periodic.
    pub clarity: f32,
    /// RMS of the analysis window.
    pub rms: f32,
}

pub struct PitchDetector {
    window: usize,
    hop: usize,
    ring: Vec<f32>,
    write: usize,
    since_hop: usize,
    fft: Arc<dyn RealToComplex<f32>>,
    ifft: Arc<dyn ComplexToReal<f32>>,
    frame: Vec<f32>,
    padded: Vec<f32>,
    spec: Vec<Complex32>,
    acf: Vec<f32>,
    nsdf: Vec<f32>,
    tau_min: usize,
    tau_max: usize,
    fs: f32,
    last: PitchEstimate,
    /// Peak-picking threshold relative to the highest key maximum.
    pub cutoff: f32,
}

impl PitchDetector {
    pub fn new(fs: f32) -> Self {
        // ~32 ms window: enough for two periods of a low male voice while
        // keeping the onset lag of the estimate small.
        let window = ((fs * 0.032) as usize).max(512) & !1;
        let hop = (window / 8).max(64);
        let n = window * 2;
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n);
        let ifft = planner.plan_fft_inverse(n);
        let spec = fft.make_output_vec();
        let tau_min = (fs / MAX_FREQ).floor() as usize;
        let tau_max = ((fs / MIN_FREQ).ceil() as usize).min(window / 2);
        Self {
            window,
            hop,
            ring: vec![0.0; window],
            write: 0,
            since_hop: 0,
            fft,
            ifft,
            frame: vec![0.0; window],
            padded: vec![0.0; n],
            spec,
            acf: vec![0.0; n],
            nsdf: vec![0.0; tau_max + 2],
            tau_min,
            tau_max,
            fs,
            last: PitchEstimate::default(),
            cutoff: 0.88,
        }
    }

    pub fn hop(&self) -> usize {
        self.hop
    }

    pub fn last(&self) -> PitchEstimate {
        self.last
    }

    /// Push one sample. Returns a fresh estimate on hop boundaries.
    #[inline]
    pub fn push(&mut self, x: f32) -> Option<PitchEstimate> {
        self.ring[self.write] = x;
        self.write += 1;
        if self.write == self.window {
            self.write = 0;
        }
        self.since_hop += 1;
        if self.since_hop >= self.hop {
            self.since_hop = 0;
            self.last = self.analyse();
            Some(self.last)
        } else {
            None
        }
    }

    fn analyse(&mut self) -> PitchEstimate {
        let w = self.window;
        // Unroll the ring oldest → newest and remove DC.
        let (a, b) = self.ring.split_at(self.write);
        self.frame[..b.len()].copy_from_slice(b);
        self.frame[b.len()..].copy_from_slice(a);
        let mean = self.frame.iter().sum::<f32>() / w as f32;
        let mut energy = 0.0f32;
        for v in self.frame.iter_mut() {
            *v -= mean;
            energy += *v * *v;
        }
        let rms = (energy / w as f32).sqrt();
        if rms < 1e-4 {
            return PitchEstimate { freq: 0.0, clarity: 0.0, rms };
        }

        self.padded[..w].copy_from_slice(&self.frame);
        self.padded[w..].fill(0.0);
        if self.fft.process(&mut self.padded, &mut self.spec).is_err() {
            return PitchEstimate { freq: 0.0, clarity: 0.0, rms };
        }
        for c in self.spec.iter_mut() {
            *c = Complex32::new(c.norm_sqr(), 0.0);
        }
        if self.ifft.process(&mut self.spec, &mut self.acf).is_err() {
            return PitchEstimate { freq: 0.0, clarity: 0.0, rms };
        }
        let scale = 1.0 / (2 * w) as f32;

        // m'(τ) = Σ x[j]² + x[j+τ]², computed incrementally.
        let mut m = 2.0 * energy;
        self.nsdf[0] = 1.0;
        for tau in 1..=self.tau_max {
            let x0 = self.frame[tau - 1];
            let x1 = self.frame[w - tau];
            m -= x0 * x0 + x1 * x1;
            self.nsdf[tau] = if m > 1e-9 { 2.0 * self.acf[tau] * scale / m } else { 0.0 };
        }

        // Key maxima: highest point of each positive lobe after the first
        // negative-going zero crossing.
        let mut maxima: [(usize, f32); 32] = [(0, 0.0); 32];
        let mut count = 0usize;
        let mut tau = 1;
        while tau < self.tau_max && self.nsdf[tau] > 0.0 {
            tau += 1;
        }
        let mut best: (usize, f32) = (0, f32::MIN);
        let mut in_lobe = false;
        while tau < self.tau_max {
            let v = self.nsdf[tau];
            if v > 0.0 {
                in_lobe = true;
                if v > best.1 {
                    best = (tau, v);
                }
            } else if in_lobe {
                in_lobe = false;
                if best.0 >= self.tau_min && count < maxima.len() {
                    maxima[count] = best;
                    count += 1;
                }
                best = (0, f32::MIN);
            }
            tau += 1;
        }
        if in_lobe && best.0 >= self.tau_min && count < maxima.len() {
            maxima[count] = best;
            count += 1;
        }
        if count == 0 {
            return PitchEstimate { freq: 0.0, clarity: 0.0, rms };
        }
        let highest = maxima[..count].iter().fold(0.0f32, |acc, m| acc.max(m.1));
        let threshold = highest * self.cutoff;
        let (t, _) = *maxima[..count].iter().find(|m| m.1 >= threshold).unwrap();

        // Parabolic interpolation around the chosen lag.
        let (y0, y1, y2) = (self.nsdf[t - 1], self.nsdf[t], self.nsdf[t + 1]);
        let denom = y0 - 2.0 * y1 + y2;
        let (delta, peak) = if denom.abs() > 1e-9 {
            let d = 0.5 * (y0 - y2) / denom;
            (d, y1 - 0.25 * (y0 - y2) * d)
        } else {
            (0.0, y1)
        };
        let period = t as f32 + delta;
        let freq = self.fs / period;
        if !(MIN_FREQ..=MAX_FREQ).contains(&freq) {
            return PitchEstimate { freq: 0.0, clarity: peak.max(0.0), rms };
        }
        PitchEstimate { freq, clarity: peak.clamp(0.0, 1.0), rms }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(freq: f32, fs: f32) -> PitchEstimate {
        let mut d = PitchDetector::new(fs);
        let mut est = PitchEstimate::default();
        for i in 0..(fs as usize / 4) {
            let t = i as f32 / fs;
            // Voice-like: fundamental plus decaying harmonics.
            let x = (1..8)
                .map(|h| (2.0 * std::f32::consts::PI * freq * h as f32 * t).sin() / h as f32)
                .sum::<f32>()
                * 0.3;
            if let Some(e) = d.push(x) {
                est = e;
            }
        }
        est
    }

    #[test]
    fn detects_across_vocal_range() {
        for &f in &[82.4, 110.0, 196.0, 261.6, 440.0, 659.3, 987.8] {
            let e = detect(f, 48_000.0);
            let cents = 1200.0 * (e.freq / f).log2();
            assert!(cents.abs() < 5.0, "f={f} got {} ({cents} cents)", e.freq);
            assert!(e.clarity > 0.9);
        }
    }

    #[test]
    fn noise_is_unvoiced_or_unclear() {
        let mut d = PitchDetector::new(48_000.0);
        let mut seed = 1u32;
        let mut est = PitchEstimate::default();
        for _ in 0..12_000 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let x = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            if let Some(e) = d.push(x * 0.5) {
                est = e;
            }
        }
        assert!(est.freq == 0.0 || est.clarity < 0.6, "{est:?}");
    }
}
