//! AI vocal separation with an MDX-Net model (UVR "Inst HQ 3") on ONNX
//! Runtime.
//!
//! The model maps the complex spectrogram of a stereo mix to the
//! spectrogram of its instrumental. Audio is processed in overlapping
//! chunks; each chunk's edges are discarded (`trim`) so seams are inaudible,
//! and finished segments are handed to the caller in order, which lets
//! playback start long before the whole song is done.

#[cfg(has_ort)]
use ort::session::builder::GraphOptimizationLevel;
#[cfg(has_ort)]
use ort::session::Session;
#[cfg(has_ort)]
use ort::value::Tensor;
use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::path::Path;
use std::sync::Arc;

/// Whether this build can run the AI model (false on Intel Macs).
pub const AVAILABLE: bool = cfg!(has_ort);

/// The model works on 44.1 kHz stereo.
pub const MODEL_RATE: u32 = 44_100;

/// Hyper-parameters of UVR-MDX-NET-Inst_HQ_3 (from UVR's model table).
#[derive(Clone, Copy, Debug)]
pub struct MdxConfig {
    pub n_fft: usize,
    pub hop: usize,
    pub dim_f: usize,
    pub dim_t: usize,
    /// Level correction UVR applies to this model's output.
    pub compensate: f32,
}

impl MdxConfig {
    /// UVR-MDX-NET-Inst_HQ_3, as listed in UVR's model table.
    pub const INST_HQ_3: Self = Self { n_fft: 6144, hop: 1024, dim_f: 3072, dim_t: 256, compensate: 1.022 };
}

/// The separation model shipped with Open Karaoke.
pub struct ModelInfo {
    pub file: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub config: MdxConfig,
}

pub const MODEL: ModelInfo = ModelInfo {
    file: "UVR-MDX-NET-Inst_HQ_3.onnx",
    url: "https://github.com/TRvlvr/model_repo/releases/download/all_public_uvr_models/UVR-MDX-NET-Inst_HQ_3.onnx",
    sha256: "317554b07fe1ea5279a77f2b1520a41ea4b93432560c4ffd08792c30fddf9adc",
    config: MdxConfig::INST_HQ_3,
};

impl MdxConfig {
    /// Samples per model chunk.
    pub fn chunk(&self) -> usize {
        self.hop * (self.dim_t - 1)
    }
    /// Samples discarded at each edge of a chunk.
    pub fn trim(&self) -> usize {
        self.n_fft / 2
    }
    /// New samples produced per chunk.
    pub fn step(&self) -> usize {
        self.chunk() - 2 * self.trim()
    }
}

/// A short-time Fourier transform matching `torch.stft`/`torch.istft` with
/// `center=True`, reflect padding and a periodic Hann window, which is what
/// MDX-Net models were trained with.
pub struct Stft {
    n_fft: usize,
    hop: usize,
    window: Vec<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
    ifft: Arc<dyn ComplexToReal<f32>>,
    frame: Vec<f32>,
    spec: Vec<Complex32>,
}

impl Stft {
    pub fn new(n_fft: usize, hop: usize) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n_fft);
        let ifft = planner.plan_fft_inverse(n_fft);
        let window = (0..n_fft)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n_fft as f32).cos())
            .collect();
        let spec = fft.make_output_vec();
        Self { n_fft, hop, window, fft, ifft, frame: vec![0.0; n_fft], spec }
    }

    pub fn bins(&self) -> usize {
        self.n_fft / 2 + 1
    }

    /// Frames for a signal of `len` samples.
    pub fn frames(&self, len: usize) -> usize {
        1 + len / self.hop
    }

    /// Forward transform; calls `out(frame, bins)` for each frame.
    pub fn forward(&mut self, x: &[f32], mut out: impl FnMut(usize, &[Complex32])) {
        let pad = self.n_fft / 2;
        let len = x.len() as isize;
        let reflect = |j: isize| -> f32 {
            let mut i = j - pad as isize;
            if i < 0 {
                i = -i;
            }
            if i >= len {
                i = 2 * (len - 1) - i;
            }
            x[i.clamp(0, len - 1) as usize]
        };
        for t in 0..self.frames(x.len()) {
            let start = (t * self.hop) as isize;
            for (k, v) in self.frame.iter_mut().enumerate() {
                *v = reflect(start + k as isize) * self.window[k];
            }
            let _ = self.fft.process(&mut self.frame, &mut self.spec);
            out(t, &self.spec);
        }
    }

    /// Inverse transform of `frames` frames into `len` samples. `get(frame,
    /// spec)` must fill the full one-sided spectrum of each frame.
    pub fn inverse(&mut self, frames: usize, len: usize, mut get: impl FnMut(usize, &mut [Complex32])) -> Vec<f32> {
        let n = self.n_fft;
        let pad = n / 2;
        let total = (frames - 1) * self.hop + n;
        let mut acc = vec![0f32; total];
        let mut env = vec![0f32; total];
        for t in 0..frames {
            get(t, &mut self.spec);
            // A real signal's DC and Nyquist bins are purely real.
            self.spec[0].im = 0.0;
            let last = self.spec.len() - 1;
            self.spec[last].im = 0.0;
            let _ = self.ifft.process(&mut self.spec, &mut self.frame);
            let off = t * self.hop;
            for k in 0..n {
                let w = self.window[k];
                acc[off + k] += self.frame[k] / n as f32 * w;
                env[off + k] += w * w;
            }
        }
        (0..len)
            .map(|i| {
                let e = env[i + pad];
                if e > 1e-8 {
                    acc[i + pad] / e
                } else {
                    0.0
                }
            })
            .collect()
    }
}

#[cfg(has_ort)]
pub struct Separator {
    session: Session,
    /// Must outlive the session.
    _env: ort::environment::Environment,
    cfg: MdxConfig,
    stft: Stft,
}

#[cfg(has_ort)]
impl Separator {
    pub fn load(model: &Path, spec: MdxConfig) -> Result<Self, String> {
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        let env = ort::init()
            .with_name("open-karaoke")
            .build()
            .map_err(|e| format!("Could not start ONNX Runtime: {e}"))?;
        let fail = |e: &dyn std::fmt::Display| format!("Could not load the vocal separation model: {e}");
        let session = Session::builder(&env)
            .map_err(|e| fail(&e))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| fail(&e))?
            .with_intra_threads(threads)
            .map_err(|e| fail(&e))?
            // Keep peak memory low enough for 8 GB machines: the default arena
            // and pre-planned memory patterns hold gigabytes for this model.
            .with_memory_pattern(false)
            .map_err(|e| fail(&e))?
            .with_allocator(
                ort::memory::MemoryInfo::new(
                    ort::memory::AllocationDevice::CPU,
                    0,
                    ort::memory::AllocatorType::Device,
                    ort::memory::MemoryType::Default,
                )
                .map_err(|e| fail(&e))?,
            )
            .map_err(|e| fail(&e))?
            .commit_from_file(model)
            .map_err(|e| fail(&e))?;
        let mut cfg = spec;
        // Trust the model's own declared shape over the defaults.
        if let Some(shape) = session.inputs().first().and_then(|i| i.dtype().tensor_shape()) {
            if shape.len() == 4 && shape[2] > 0 && shape[3] > 0 {
                cfg.dim_f = shape[2] as usize;
                cfg.dim_t = shape[3] as usize;
            }
        }
        let stft = Stft::new(cfg.n_fft, cfg.hop);
        Ok(Self { session, _env: env, cfg, stft })
    }

    pub fn config(&self) -> MdxConfig {
        self.cfg
    }

    fn infer(&mut self, input: &[f32]) -> Result<Vec<f32>, String> {
        let cfg = self.cfg;
        let tensor = Tensor::from_array(([1usize, 4, cfg.dim_f, cfg.dim_t], input.to_vec())).map_err(|e| e.to_string())?;
        let outputs = self.session.run(ort::inputs![tensor]).map_err(|e| format!("Separation failed: {e}"))?;
        let (_, out) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        if out.len() < 4 * cfg.dim_f * cfg.dim_t {
            return Err("The separation model returned an unexpected shape".into());
        }
        Ok(out.to_vec())
    }

    /// Separate interleaved stereo at [`MODEL_RATE`]. `on_segment` receives
    /// consecutive pieces of the instrumental (interleaved stereo) and the
    /// fraction done; return `false` from it to cancel.
    pub fn run(&mut self, mix: &[f32], mut on_segment: impl FnMut(&[f32], f32) -> bool) -> Result<(), String> {
        let cfg = self.cfg;
        let n = mix.len() / 2;
        if n == 0 {
            return Ok(());
        }
        let (chunk, trim, step) = (cfg.chunk(), cfg.trim(), cfg.step());
        let frames = self.stft.frames(chunk);
        if frames != cfg.dim_t {
            return Err(format!("Unexpected model shape: {} frames, model wants {}", frames, cfg.dim_t));
        }
        let bins = self.stft.bins();
        let plane = cfg.dim_f * cfg.dim_t;

        // Planar channels padded with `trim` silence in front and enough at
        // the back to fill the last chunk.
        let pad_end = step - n % step + trim;
        let padded_len = trim + n + pad_end;
        let mut planar = [vec![0f32; padded_len], vec![0f32; padded_len]];
        for i in 0..n {
            planar[0][trim + i] = mix[i * 2];
            planar[1][trim + i] = mix[i * 2 + 1];
        }

        let chunks = n.div_ceil(step);
        let mut input = vec![0f32; 4 * plane];
        let mut seg = Vec::with_capacity(step * 2);
        for c in 0..chunks {
            let start = c * step;
            // Spectrogram → [1, 4, dim_f, dim_t]: (ch0 re, ch0 im, ch1 re, ch1 im).
            for ch in 0..2 {
                let x = &planar[ch][start..start + chunk];
                let base_re = (ch * 2) * plane;
                let base_im = (ch * 2 + 1) * plane;
                self.stft.forward(x, |t, spec| {
                    for f in 0..cfg.dim_f {
                        input[base_re + f * cfg.dim_t + t] = spec[f].re;
                        input[base_im + f * cfg.dim_t + t] = spec[f].im;
                    }
                });
            }
            let out = self.infer(&input)?;

            // Back to audio, keeping only the trimmed centre of the chunk.
            let mut chans: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
            for (ch, dst) in chans.iter_mut().enumerate() {
                let base_re = (ch * 2) * plane;
                let base_im = (ch * 2 + 1) * plane;
                *dst = self.stft.inverse(frames, chunk, |t, spec| {
                    for (f, s) in spec.iter_mut().enumerate().take(bins) {
                        *s = if f < cfg.dim_f {
                            Complex32::new(out[base_re + f * cfg.dim_t + t], out[base_im + f * cfg.dim_t + t])
                        } else {
                            Complex32::new(0.0, 0.0)
                        };
                    }
                });
            }
            let keep = step.min(n - start);
            seg.clear();
            for i in 0..keep {
                seg.push(chans[0][trim + i] * cfg.compensate);
                seg.push(chans[1][trim + i] * cfg.compensate);
            }
            if !on_segment(&seg, (c + 1) as f32 / chunks as f32) {
                return Err("cancelled".into());
            }
        }
        Ok(())
    }
}

/// Stand-in for builds without ONNX Runtime; callers check [`AVAILABLE`].
#[cfg(not(has_ort))]
pub struct Separator {
    cfg: MdxConfig,
}

#[cfg(not(has_ort))]
impl Separator {
    pub fn load(_model: &Path, _spec: MdxConfig) -> Result<Self, String> {
        Err(UNAVAILABLE.into())
    }

    pub fn config(&self) -> MdxConfig {
        self.cfg
    }

    pub fn run(&mut self, _mix: &[f32], _on_segment: impl FnMut(&[f32], f32) -> bool) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
}

pub const UNAVAILABLE: &str = "AI separation isn't available on Intel Macs yet";

// ------------------------------------------------------- growing buffer --

/// A fixed-size sample buffer filled front to back by one writer while the
/// audio thread reads the part already written. Samples below the `ready`
/// watermark are never written again, so readers need no lock.
pub struct Growing {
    data: Box<[std::cell::UnsafeCell<f32>]>,
    ready: std::sync::atomic::AtomicUsize,
    done: std::sync::atomic::AtomicBool,
}

// SAFETY: the single `GrowingWriter` only writes at indices >= `ready` and
// publishes them with a Release store; readers only touch indices < `ready`
// observed with an Acquire load, so no sample is accessed concurrently.
unsafe impl Sync for Growing {}
unsafe impl Send for Growing {}

impl Growing {
    /// Create a buffer of `len` samples and its unique writer.
    pub fn new(len: usize) -> (Arc<Self>, GrowingWriter) {
        let data = (0..len).map(|_| std::cell::UnsafeCell::new(0.0)).collect();
        let g = Arc::new(Self {
            data,
            ready: std::sync::atomic::AtomicUsize::new(0),
            done: std::sync::atomic::AtomicBool::new(false),
        });
        (g.clone(), GrowingWriter { buf: g })
    }

    pub fn capacity(&self) -> usize {
        self.data.len()
    }

    pub fn ready(&self) -> usize {
        self.ready.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn is_done(&self) -> bool {
        self.done.load(std::sync::atomic::Ordering::Acquire)
    }

    /// The finished prefix.
    pub fn ready_slice(&self) -> &[f32] {
        let n = self.ready();
        // SAFETY: see the `Sync` impl; `UnsafeCell<f32>` has f32's layout.
        unsafe { std::slice::from_raw_parts(self.data.as_ptr() as *const f32, n) }
    }
}

/// The only handle allowed to write into a [`Growing`] buffer.
pub struct GrowingWriter {
    buf: Arc<Growing>,
}

impl GrowingWriter {
    /// Append samples; anything beyond the capacity is dropped.
    pub fn append(&mut self, s: &[f32]) {
        let start = self.buf.ready.load(std::sync::atomic::Ordering::Relaxed);
        let n = s.len().min(self.buf.data.len() - start);
        for (i, &v) in s[..n].iter().enumerate() {
            // SAFETY: index >= ready, so no reader can be looking at it.
            unsafe { *self.buf.data[start + i].get() = v };
        }
        self.buf.ready.store(start + n, std::sync::atomic::Ordering::Release);
    }

    pub fn finish(self) {
        // Treat the rest as silence; readers may play it immediately.
        let cap = self.buf.data.len();
        self.buf.ready.store(cap, std::sync::atomic::Ordering::Release);
        self.buf.done.store(true, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stft_round_trip_is_transparent() {
        let cfg = MdxConfig::INST_HQ_3;
        let mut st = Stft::new(cfg.n_fft, cfg.hop);
        let len = cfg.chunk();
        let x: Vec<f32> = (0..len)
            .map(|i| ((i as f32 * 0.013).sin() + (i as f32 * 0.0021).cos() * 0.5) * 0.4)
            .collect();
        let frames = st.frames(len);
        assert_eq!(frames, cfg.dim_t);
        let mut spec = vec![vec![Complex32::new(0.0, 0.0); st.bins()]; frames];
        st.forward(&x, |t, s| spec[t].copy_from_slice(s));
        let y = st.inverse(frames, len, |t, s| s.copy_from_slice(&spec[t]));
        let err = x.iter().zip(&y).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(err < 1e-4, "max error {err}");
    }

    #[test]
    fn growing_buffer_publishes_in_order() {
        let (g, mut w) = Growing::new(10);
        assert_eq!(g.ready_slice().len(), 0);
        w.append(&[1.0, 2.0, 3.0]);
        assert_eq!(g.ready_slice(), &[1.0, 2.0, 3.0]);
        w.append(&[4.0; 20]);
        assert_eq!(g.ready_slice().len(), 10);
        w.finish();
        assert!(g.is_done());
    }
}
