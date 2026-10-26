//! Offline song analysis: musical key detection and a basic centre-channel
//! vocal remover (the fallback when AI separation can't run).

use super::fx::{Biquad, Shape};
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
}
