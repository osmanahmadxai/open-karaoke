//! Offline song analysis: musical key, lyric alignment and a basic
//! centre-channel vocal remover.

use super::fx::{Biquad, Shape};
use realfft::num_complex::Complex32;
use realfft::RealFftPlanner;
use serde::Serialize;

pub fn mono(stereo: &[f32]) -> Vec<f32> {
    stereo.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect()
}

#[derive(Clone, Copy, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct KeyEstimate {
    /// Pitch class of the tonic, 0 = C.
    pub tonic: u8,
    pub minor: bool,
    /// 0..1, gap between the best and runner-up correlation.
    pub confidence: f32,
}

/// Krumhansl–Kessler key-finding over a whole-song chromagram.
pub fn detect_key(mono: &[f32], fs: u32) -> KeyEstimate {
    const MAJOR: [f32; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
    const MINOR: [f32; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
    let n = 8192;
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let mut frame = fft.make_input_vec();
    let mut spec = fft.make_output_vec();
    let window: Vec<f32> = (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect();
    // Precompute the pitch class of each useful bin (≈ 55 Hz – 2 kHz).
    let bin_pc: Vec<Option<usize>> = (0..n / 2 + 1)
        .map(|k| {
            let f = k as f32 * fs as f32 / n as f32;
            if (55.0..2000.0).contains(&f) {
                Some(((12.0 * (f / 440.0).log2() + 69.0).round() as i32).rem_euclid(12) as usize)
            } else {
                None
            }
        })
        .collect();

    let mut chroma = [0f32; 12];
    let mut start = 0;
    while start + n <= mono.len() {
        for i in 0..n {
            frame[i] = mono[start + i] * window[i];
        }
        if fft.process(&mut frame, &mut spec).is_ok() {
            let mut local = [0f32; 12];
            for (k, c) in spec.iter().enumerate() {
                if let Some(pc) = bin_pc[k] {
                    local[pc] += c.norm();
                }
            }
            // Normalise per frame so loud sections don't dominate.
            let sum: f32 = local.iter().sum();
            if sum > 1e-6 {
                for (a, b) in chroma.iter_mut().zip(local) {
                    *a += b / sum;
                }
            }
        }
        start += n;
    }

    fn corr(a: &[f32; 12], b: &[f32; 12], rot: usize) -> f32 {
        let ma = a.iter().sum::<f32>() / 12.0;
        let mb = b.iter().sum::<f32>() / 12.0;
        let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
        for i in 0..12 {
            let x = a[(i + rot) % 12] - ma;
            let y = b[i] - mb;
            num += x * y;
            da += x * x;
            db += y * y;
        }
        num / (da * db).sqrt().max(1e-9)
    }

    let mut scores: Vec<(f32, u8, bool)> = Vec::with_capacity(24);
    for tonic in 0..12 {
        scores.push((corr(&chroma, &MAJOR, tonic), tonic as u8, false));
        scores.push((corr(&chroma, &MINOR, tonic), tonic as u8, true));
    }
    scores.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let (best, tonic, minor) = scores[0];
    KeyEstimate { tonic, minor, confidence: ((best - scores[1].0) * 4.0).clamp(0.0, 1.0) }
}

const ENV_HOP: usize = 512;

/// Log-magnitude spectral flux, mean-removed and unit-variance.
fn onset_envelope(mono: &[f32]) -> Vec<f32> {
    let n = 2048;
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let mut frame = fft.make_input_vec();
    let mut spec = fft.make_output_vec();
    let window: Vec<f32> = (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect();
    let mut prev = vec![0f32; n / 2 + 1];
    let mut env = Vec::with_capacity(mono.len() / ENV_HOP + 1);
    let mut start = 0;
    while start + n <= mono.len() {
        for i in 0..n {
            frame[i] = mono[start + i] * window[i];
        }
        let _ = fft.process(&mut frame, &mut spec);
        let mut flux = 0.0;
        for (k, c) in spec.iter().enumerate() {
            let m = (1.0 + 100.0 * c.norm()).ln();
            flux += (m - prev[k]).max(0.0);
            prev[k] = m;
        }
        env.push(flux);
        start += ENV_HOP;
    }
    // Remove slow trends (local mean over ~1.4 s), then standardise.
    let w = 64;
    let mut out = vec![0f32; env.len()];
    let mut acc = 0.0f64;
    for i in 0..env.len() {
        acc += env[i] as f64;
        if i >= w {
            acc -= env[i - w] as f64;
        }
        let mean = acc / (i.min(w - 1) + 1) as f64;
        out[i] = (env[i] as f64 - mean).max(0.0) as f32;
    }
    let mean = out.iter().sum::<f32>() / out.len().max(1) as f32;
    let var = out.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / out.len().max(1) as f32;
    let sd = var.sqrt().max(1e-9);
    out.iter().map(|x| (x - mean) / sd).collect()
}

#[derive(Clone, Copy, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Alignment {
    /// Seconds to add to a time in the original recording to get the
    /// matching time in the instrumental.
    pub offset: f64,
    /// Normalised correlation at the peak, roughly 0..1.
    pub confidence: f32,
}

/// Find where the original recording sits inside the instrumental by
/// cross-correlating their rhythmic onset envelopes.
pub fn align(instrumental: &[f32], original: &[f32], fs: u32, max_offset_s: f64) -> Option<Alignment> {
    let a = onset_envelope(instrumental);
    let b = onset_envelope(original);
    if a.len() < 64 || b.len() < 64 {
        return None;
    }
    let fps = fs as f64 / ENV_HOP as f64;
    let n = (a.len() + b.len()).next_power_of_two();
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let ifft = planner.plan_fft_inverse(n);
    let mut pa = vec![0f32; n];
    let mut pb = vec![0f32; n];
    pa[..a.len()].copy_from_slice(&a);
    pb[..b.len()].copy_from_slice(&b);
    let mut sa = fft.make_output_vec();
    let mut sb = fft.make_output_vec();
    fft.process(&mut pa, &mut sa).ok()?;
    fft.process(&mut pb, &mut sb).ok()?;
    let mut prod: Vec<Complex32> = sa.iter().zip(&sb).map(|(x, y)| x * y.conj()).collect();
    let mut corr = vec![0f32; n];
    ifft.process(&mut prod, &mut corr).ok()?;

    let max_lag = ((max_offset_s * fps) as usize).min(n / 2 - 1);
    let overlap = a.len().min(b.len()) as f32;
    let mut best = (0i64, f32::MIN);
    let mut stats = (0.0f64, 0.0f64, 0usize);
    for lag in -(max_lag as i64)..=(max_lag as i64) {
        let idx = lag.rem_euclid(n as i64) as usize;
        let v = corr[idx] / n as f32 / overlap;
        stats.0 += v as f64;
        stats.1 += (v * v) as f64;
        stats.2 += 1;
        if v > best.1 {
            best = (lag, v);
        }
    }
    // Sub-frame refinement.
    let at = |lag: i64| corr[lag.rem_euclid(n as i64) as usize];
    let (y0, y1, y2) = (at(best.0 - 1), at(best.0), at(best.0 + 1));
    let d = y0 - 2.0 * y1 + y2;
    let frac = if d.abs() > 1e-9 { 0.5 * (y0 - y2) / d } else { 0.0 };
    let mean = stats.0 / stats.2 as f64;
    let sd = (stats.1 / stats.2 as f64 - mean * mean).max(1e-12).sqrt();
    let z = ((best.1 as f64 - mean) / sd) as f32;
    Some(Alignment {
        offset: (best.0 as f64 + frac as f64) / fps,
        // A sharp, isolated peak (z ≳ 8) is a confident match.
        confidence: ((z - 4.0) / 8.0).clamp(0.0, 1.0),
    })
}

/// Basic karaoke effect: removes centre-panned content (usually the lead
/// vocal) while keeping the bass and kick, which are also centred.
pub fn remove_center(stereo: &[f32], fs: u32) -> Vec<f32> {
    let mut lp1 = Biquad::default();
    let mut lp2 = Biquad::default();
    lp1.set(Shape::LowPass, 140.0, 0.707, fs as f32);
    lp2.set(Shape::LowPass, 140.0, 0.707, fs as f32);
    let mut out = Vec::with_capacity(stereo.len());
    for f in stereo.chunks_exact(2) {
        let mid = (f[0] + f[1]) * 0.5;
        let side = (f[0] - f[1]) * 0.5;
        let bass = lp2.run(lp1.run(mid));
        out.push(bass + side * 1.4);
        out.push(bass - side * 1.4);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freqs: &[f32], secs: f32, fs: u32) -> Vec<f32> {
        (0..(secs * fs as f32) as usize)
            .map(|i| {
                let t = i as f32 / fs as f32;
                freqs.iter().map(|f| (std::f32::consts::TAU * f * t).sin()).sum::<f32>() * 0.1
            })
            .collect()
    }

    #[test]
    fn finds_a_minor_from_triads() {
        let fs = 22_050;
        // i – iv – v – i in A minor.
        let mut sig = tone(&[220.0, 261.63, 329.63], 2.0, fs);
        sig.extend(tone(&[293.66, 349.23, 440.0], 2.0, fs));
        sig.extend(tone(&[329.63, 392.0, 493.88], 2.0, fs));
        sig.extend(tone(&[220.0, 261.63, 329.63], 2.0, fs));
        let k = detect_key(&sig, fs);
        assert_eq!((k.tonic, k.minor), (9, true), "{k:?}");
    }

    #[test]
    fn aligns_shifted_rhythm() {
        let fs = 22_050u32;
        let mut seed = 7u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        // An irregular pattern of clicks so there is exactly one true lag.
        let len = fs as usize * 40;
        let mut original = vec![0f32; len];
        let mut t = 0usize;
        while t < len - 2000 {
            for i in 0..800 {
                original[t + i] += (rnd() - 0.5) * (-(i as f32) / 120.0).exp();
            }
            t += (fs as f32 * (0.2 + rnd() * 0.5)) as usize;
        }
        let shift = (fs as f32 * 2.75) as usize;
        let mut inst = vec![0f32; len + shift];
        inst[shift..].copy_from_slice(&original);
        let a = align(&inst, &original, fs, 20.0).unwrap();
        assert!((a.offset - 2.75).abs() < 0.03, "{a:?}");
        assert!(a.confidence > 0.5, "{a:?}");
    }
}
