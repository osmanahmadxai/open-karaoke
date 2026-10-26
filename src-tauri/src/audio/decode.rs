//! Decode an audio file to interleaved stereo f32 at a target sample rate.

use rubato::{FftFixedIn, Resampler};
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub fn decode_file(path: &Path, target_sr: u32) -> Result<Vec<f32>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("Unsupported audio format: {e}"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("No audio track in file")?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("Unsupported codec: {e}"))?;
    let mut src_sr = track.codec_params.sample_rate.unwrap_or(44_100);

    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut sbuf: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(format!("Read error: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(format!("Decode error: {e}")),
        };
        let spec = *decoded.spec();
        src_sr = spec.rate;
        let ch = spec.channels.count().max(1);
        let need = decoded.capacity() as u64;
        if sbuf.as_ref().map(|b| b.capacity() < need as usize * ch).unwrap_or(true) {
            sbuf = Some(SampleBuffer::<f32>::new(need, spec));
        }
        let buf = sbuf.as_mut().unwrap();
        buf.copy_interleaved_ref(decoded);
        for frame in buf.samples().chunks(ch) {
            left.push(frame[0]);
            right.push(if ch > 1 { frame[1] } else { frame[0] });
        }
    }
    if left.is_empty() {
        return Err("The audio file contained no samples".into());
    }

    let (left, right) = if src_sr != target_sr {
        resample(&[left, right], src_sr, target_sr)?
    } else {
        (left, right)
    };
    let mut out = Vec::with_capacity(left.len() * 2);
    for (l, r) in left.iter().zip(&right) {
        out.push(*l);
        out.push(*r);
    }
    Ok(out)
}

fn resample(chans: &[Vec<f32>; 2], from: u32, to: u32) -> Result<(Vec<f32>, Vec<f32>), String> {
    let chunk = 4096;
    let mut rs = FftFixedIn::<f32>::new(from as usize, to as usize, chunk, 2, 2).map_err(|e| e.to_string())?;
    let n = chans[0].len();
    let expected = (n as f64 * to as f64 / from as f64).round() as usize;
    let delay = rs.output_delay();
    let mut out = [Vec::with_capacity(expected + chunk), Vec::with_capacity(expected + chunk)];
    // Always feed whole chunks: the remainder and the filter-delay flush are
    // zero-padded rather than passed as short buffers.
    let mut pad = [vec![0f32; chunk], vec![0f32; chunk]];
    let mut i = 0;
    while out[0].len() < expected + delay {
        let block = if i + chunk <= n {
            rs.process(&[&chans[0][i..i + chunk], &chans[1][i..i + chunk]], None)
        } else {
            for ch in 0..2 {
                pad[ch].fill(0.0);
                if i < n {
                    pad[ch][..n - i].copy_from_slice(&chans[ch][i..]);
                }
            }
            rs.process(&[&pad[0][..], &pad[1][..]], None)
        }
        .map_err(|e| e.to_string())?;
        out[0].extend_from_slice(&block[0]);
        out[1].extend_from_slice(&block[1]);
        i += chunk;
    }
    let [mut l, mut r] = out;
    l.drain(..delay.min(l.len()));
    r.drain(..delay.min(r.len()));
    l.truncate(expected);
    r.truncate(expected);
    Ok((l, r))
}

/// Resamples interleaved stereo delivered in arbitrary-sized pieces, as the
/// vocal separator produces it. A no-op when the rates already match.
pub struct StreamResampler {
    rs: Option<FftFixedIn<f32>>,
    pending: [Vec<f32>; 2],
    skip: usize,
    chunk: usize,
}

impl StreamResampler {
    pub fn new(from: u32, to: u32) -> Result<Self, String> {
        let chunk = 2048;
        let rs = if from == to {
            None
        } else {
            Some(FftFixedIn::<f32>::new(from as usize, to as usize, chunk, 2, 2).map_err(|e| e.to_string())?)
        };
        let skip = rs.as_ref().map(|r| r.output_delay()).unwrap_or(0);
        Ok(Self { rs, pending: [Vec::new(), Vec::new()], skip, chunk })
    }

    /// Feed interleaved stereo; `out` receives whatever is ready.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<(), String> {
        out.clear();
        if self.rs.is_none() {
            out.extend_from_slice(input);
            return Ok(());
        }
        for f in input.chunks_exact(2) {
            self.pending[0].push(f[0]);
            self.pending[1].push(f[1]);
        }
        self.drain(out)
    }

    /// Push silence through to release the filter's delayed tail.
    pub fn flush(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
        out.clear();
        if self.rs.is_none() {
            return Ok(());
        }
        let delay = self.rs.as_ref().map(|r| r.output_delay()).unwrap_or(0);
        let pad = self.chunk * 2 + delay;
        for ch in self.pending.iter_mut() {
            ch.extend(std::iter::repeat(0.0).take(pad));
        }
        self.drain(out)
    }

    fn drain(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
        let rs = self.rs.as_mut().unwrap();
        while self.pending[0].len() >= self.chunk {
            let block = rs
                .process(&[&self.pending[0][..self.chunk], &self.pending[1][..self.chunk]], None)
                .map_err(|e| e.to_string())?;
            for ch in self.pending.iter_mut() {
                ch.drain(..self.chunk);
            }
            for i in 0..block[0].len() {
                if self.skip > 0 {
                    self.skip -= 1;
                    continue;
                }
                out.push(block[0][i]);
                out.push(block[1][i]);
            }
        }
        Ok(())
    }
}

/// Save interleaved stereo as 16-bit WAV, atomically (write, then rename).
pub fn write_wav(path: &Path, stereo: &[f32], rate: u32) -> Result<(), String> {
    let tmp = path.with_extension("part");
    let spec = hound::WavSpec { channels: 2, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(&tmp, spec).map_err(|e| e.to_string())?;
    for &s in stereo {
        w.write_sample((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_resampler_keeps_length_and_pitch() {
        let (from, to) = (44_100u32, 48_000u32);
        let n = from as usize * 3;
        let input: Vec<f32> = (0..n).flat_map(|i| {
            let v = (std::f32::consts::TAU * 440.0 * i as f32 / from as f32).sin() * 0.5;
            [v, v]
        }).collect();
        let mut rs = StreamResampler::new(from, to).unwrap();
        let (mut out, mut all) = (Vec::new(), Vec::new());
        // Uneven piece sizes, like separator segments.
        for piece in input.chunks(2 * 12_345) {
            rs.push(piece, &mut out).unwrap();
            all.extend_from_slice(&out);
        }
        rs.flush(&mut out).unwrap();
        all.extend_from_slice(&out);
        let expected = n * to as usize / from as usize;
        assert!((all.len() / 2) >= expected, "{} < {expected}", all.len() / 2);
        // Zero crossings over 1 s in the middle → 440 Hz.
        let mid = &all[2 * to as usize..4 * to as usize];
        let crossings = mid.chunks(2).collect::<Vec<_>>().windows(2).filter(|w| (w[0][0] < 0.0) != (w[1][0] < 0.0)).count();
        assert!((crossings as i32 - 880).abs() <= 4, "{crossings}");
    }
}
