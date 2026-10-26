//! Real-time audio I/O.
//!
//! The microphone is processed inside the input callback and handed to the
//! output callback through a lock-free ring; the output callback mixes it
//! with the backing track. Nothing on either audio thread locks or allocates
//! in the steady state. cpal streams are not `Send` on every platform, so a
//! dedicated host thread owns them and rebuilds them on device changes.

use super::chain::{PitchFrame, VocalChain};
use super::fx::Limiter;
use super::params::{AtomicF32, Flags, Params};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use crossbeam_channel::{bounded, Receiver, Sender};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::thread;

/// Interleaved stereo samples: complete, or still being produced by the
/// vocal separator while playback has already started.
#[derive(Clone)]
pub enum Buf {
    Full(Arc<Vec<f32>>),
    Growing(Arc<super::separate::Growing>),
}

impl Buf {
    /// Samples that can be played right now.
    #[inline]
    pub fn ready(&self) -> &[f32] {
        match self {
            Buf::Full(v) => v,
            Buf::Growing(g) => g.ready_slice(),
        }
    }

    pub fn total_samples(&self) -> usize {
        match self {
            Buf::Full(v) => v.len(),
            Buf::Growing(g) => g.capacity(),
        }
    }
}

/// A backing track ready for playback at the engine sample rate.
pub struct Track {
    pub audio: Buf,
    /// Optional vocal guide (the isolated original singer).
    pub guide: Option<Buf>,
    /// `track_frame = guide_frame + guide_offset`.
    pub guide_offset: i64,
}

impl Track {
    pub fn frames(&self) -> u64 {
        (self.audio.total_samples() / 2) as u64
    }

    /// Frames playable now (less than `frames()` while still separating).
    pub fn ready_frames(&self) -> u64 {
        (self.audio.ready().len() / 2) as u64
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StampedFrame {
    pub frame: PitchFrame,
    /// Song position (frames) when the frame was analysed.
    pub pos: u64,
}

enum Cmd {
    SetTrack(Option<Arc<Track>>, u64),
    Seek(u64),
    Recorder(Option<HeapProd<f32>>),
}

/// Voice Lab commands for the input thread.
enum LabCmd {
    /// Start capturing the raw mic into this (pre-allocated) buffer.
    Record(Vec<f32>),
    /// Stop capturing and send the buffer back.
    StopRecord,
    /// Feed a recorded take through the vocal chain instead of the mic.
    Play(Arc<Vec<f32>>, bool),
    StopPlay,
}

pub const LAB_IDLE: u32 = 0;
pub const LAB_RECORDING: u32 = 1;
pub const LAB_PLAYING: u32 = 2;
/// Longest Voice Lab take, seconds.
pub const LAB_MAX_SECONDS: usize = 30;

pub struct Shared {
    pub params: Params,
    pub flags: Flags,
    pub position: AtomicU64,
    pub length: AtomicU64,
    pub sample_rate: AtomicU32,
    pub mic_peak: AtomicF32,
    pub out_peak: AtomicF32,
    pub out_latency: AtomicF32,
    pub in_latency: AtomicF32,
    pub chain_latency: AtomicU32,
    pub ended: AtomicBool,
    /// Playback caught up with the vocal separator and is waiting for audio.
    pub buffering: AtomicBool,
    pub input_ok: AtomicBool,
    pub stream_error: Mutex<Option<String>>,
    pub lab_state: AtomicU32,
    pub lab_pos: AtomicU64,
    pub lab_len: AtomicU64,
    /// A/B: hear the take unprocessed.
    pub lab_bypass: AtomicBool,
}

#[derive(Clone, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DeviceList {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub default_input: Option<String>,
    pub default_output: Option<String>,
}

#[derive(Clone, Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AudioStatus {
    pub sample_rate: u32,
    pub output: String,
    pub input: Option<String>,
    pub input_error: Option<String>,
    pub buffer: Option<u32>,
    pub latency_ms: f32,
}

#[derive(Clone, Default)]
pub struct DeviceChoice {
    pub input: Option<String>,
    pub output: Option<String>,
    pub buffer: Option<u32>,
}

struct OpenReq {
    choice: DeviceChoice,
    reply: Sender<Result<AudioStatus, String>>,
}

pub struct Engine {
    pub shared: Arc<Shared>,
    cmd_tx: Sender<Cmd>,
    cmd_rx: Receiver<Cmd>,
    host_tx: Sender<OpenReq>,
    garbage_tx: Sender<Arc<Track>>,
    lab_tx: Sender<LabCmd>,
    lab_rx: Receiver<LabCmd>,
    take_tx: Sender<Vec<f32>>,
    take_rx: Receiver<Vec<f32>>,
    buf_garbage_tx: Sender<Arc<Vec<f32>>>,
    take: Mutex<Option<Arc<Vec<f32>>>>,
    current: Mutex<Option<Arc<Track>>>,
    pub frames: Mutex<Option<HeapCons<StampedFrame>>>,
    recorder: Mutex<Option<(PathBuf, thread::JoinHandle<Result<(), String>>)>>,
    status: Mutex<AudioStatus>,
}

fn device_name(d: &cpal::Device) -> String {
    d.description().map(|x| x.name().to_string()).unwrap_or_else(|_| "Unknown device".into())
}

pub fn list_devices() -> DeviceList {
    let host = cpal::default_host();
    let names = |it: Result<_, _>| -> Vec<String> {
        match it {
            Ok(iter) => {
                let mut v: Vec<String> = Vec::new();
                for d in iter {
                    let n = device_name(&d);
                    if !v.contains(&n) {
                        v.push(n);
                    }
                }
                v
            }
            Err(_) => Vec::new(),
        }
    };
    DeviceList {
        inputs: names(host.input_devices().map(|i| i.collect::<Vec<_>>().into_iter())),
        outputs: names(host.output_devices().map(|i| i.collect::<Vec<_>>().into_iter())),
        default_input: host.default_input_device().map(|d| device_name(&d)),
        default_output: host.default_output_device().map(|d| device_name(&d)),
    }
}

impl Engine {
    pub fn new() -> Arc<Self> {
        let shared = Arc::new(Shared {
            params: Params::default(),
            flags: Flags::default(),
            position: AtomicU64::new(0),
            length: AtomicU64::new(0),
            sample_rate: AtomicU32::new(48_000),
            mic_peak: AtomicF32::new(0.0),
            out_peak: AtomicF32::new(0.0),
            out_latency: AtomicF32::new(0.0),
            in_latency: AtomicF32::new(0.0),
            chain_latency: AtomicU32::new(0),
            ended: AtomicBool::new(false),
            buffering: AtomicBool::new(false),
            input_ok: AtomicBool::new(false),
            stream_error: Mutex::new(None),
            lab_state: AtomicU32::new(LAB_IDLE),
            lab_pos: AtomicU64::new(0),
            lab_len: AtomicU64::new(0),
            lab_bypass: AtomicBool::new(false),
        });
        let (cmd_tx, cmd_rx) = bounded(64);
        let (garbage_tx, garbage_rx) = bounded::<Arc<Track>>(64);
        // Tracks are dropped here, never on the audio thread.
        thread::Builder::new()
            .name("audio-gc".into())
            .spawn(move || while garbage_rx.recv().is_ok() {})
            .ok();
        let (buf_garbage_tx, buf_garbage_rx) = bounded::<Arc<Vec<f32>>>(16);
        thread::Builder::new()
            .name("audio-gc-takes".into())
            .spawn(move || while buf_garbage_rx.recv().is_ok() {})
            .ok();
        let (lab_tx, lab_rx) = bounded(16);
        let (take_tx, take_rx) = bounded(2);

        let (host_tx, host_rx) = bounded::<OpenReq>(4);
        let engine = Arc::new(Self {
            shared,
            cmd_tx,
            cmd_rx,
            host_tx,
            garbage_tx,
            lab_tx,
            lab_rx,
            take_tx,
            take_rx,
            buf_garbage_tx,
            take: Mutex::new(None),
            current: Mutex::new(None),
            frames: Mutex::new(None),
            recorder: Mutex::new(None),
            status: Mutex::new(AudioStatus::default()),
        });

        let weak = Arc::downgrade(&engine);
        thread::Builder::new()
            .name("audio-host".into())
            .spawn(move || {
                let mut streams: Option<(cpal::Stream, Option<cpal::Stream>)> = None;
                while let Ok(req) = host_rx.recv() {
                    let Some(engine) = weak.upgrade() else { break };
                    streams = None;
                    let result = engine.build_streams(&req.choice).map(|(s, status)| {
                        streams = Some(s);
                        status
                    });
                    let _ = req.reply.send(result);
                }
                drop(streams);
            })
            .expect("spawn audio host thread");
        engine
    }

    pub fn status(&self) -> AudioStatus {
        self.status.lock().unwrap().clone()
    }

    /// (Re)open audio devices. Keeps the current track and position.
    pub fn open(&self, choice: DeviceChoice) -> Result<AudioStatus, String> {
        let (tx, rx) = bounded(1);
        self.host_tx.send(OpenReq { choice, reply: tx }).map_err(|e| e.to_string())?;
        let status = rx.recv().map_err(|e| e.to_string())??;
        *self.status.lock().unwrap() = status.clone();
        let pos = self.shared.position.load(Relaxed);
        let current = self.current.lock().unwrap().clone();
        let _ = self.cmd_tx.send(Cmd::SetTrack(current, pos));
        Ok(status)
    }

    pub fn sample_rate(&self) -> u32 {
        self.shared.sample_rate.load(Relaxed)
    }

    pub fn set_track(&self, track: Option<Arc<Track>>, pos: u64) {
        self.shared.length.store(track.as_ref().map(|t| t.frames()).unwrap_or(0), Relaxed);
        self.shared.position.store(pos, Relaxed);
        self.shared.ended.store(false, Relaxed);
        *self.current.lock().unwrap() = track.clone();
        let _ = self.cmd_tx.send(Cmd::SetTrack(track, pos));
    }

    pub fn current_track(&self) -> Option<Arc<Track>> {
        self.current.lock().unwrap().clone()
    }

    pub fn play(&self) {
        if self.shared.ended.swap(false, Relaxed) {
            self.seek_frames(0);
        }
        self.shared.flags.playing.store(true, Relaxed);
    }

    pub fn pause(&self) {
        self.shared.flags.playing.store(false, Relaxed);
    }

    pub fn seek_frames(&self, frames: u64) {
        let frames = frames.min(self.shared.length.load(Relaxed));
        self.shared.position.store(frames, Relaxed);
        self.shared.ended.store(false, Relaxed);
        let _ = self.cmd_tx.send(Cmd::Seek(frames));
    }

    pub fn start_recording(&self, path: PathBuf) -> Result<(), String> {
        self.stop_recording()?;
        let sr = self.sample_rate();
        let rb = HeapRb::<f32>::new(sr as usize * 2 * 4);
        let (prod, mut cons) = rb.split();
        let spec = hound::WavSpec { channels: 2, sample_rate: sr, bits_per_sample: 24, sample_format: hound::SampleFormat::Int };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut writer = hound::WavWriter::create(&path, spec).map_err(|e| e.to_string())?;
        let shared = self.shared.clone();
        let handle = thread::Builder::new()
            .name("recorder".into())
            .spawn(move || -> Result<(), String> {
                let mut buf = vec![0.0f32; 8192];
                loop {
                    let n = cons.pop_slice(&mut buf);
                    for &s in &buf[..n] {
                        let v = (s.clamp(-1.0, 1.0) * 8_388_607.0) as i32;
                        writer.write_sample(v).map_err(|e| e.to_string())?;
                    }
                    if n == 0 {
                        if !shared.flags.recording.load(Relaxed) && cons.is_empty() {
                            break;
                        }
                        thread::sleep(std::time::Duration::from_millis(20));
                    }
                }
                writer.finalize().map_err(|e| e.to_string())
            })
            .map_err(|e| e.to_string())?;
        self.cmd_tx.send(Cmd::Recorder(Some(prod))).map_err(|e| e.to_string())?;
        self.shared.flags.recording.store(true, Relaxed);
        *self.recorder.lock().unwrap() = Some((path, handle));
        Ok(())
    }

    pub fn stop_recording(&self) -> Result<Option<PathBuf>, String> {
        let Some((path, handle)) = self.recorder.lock().unwrap().take() else {
            return Ok(None);
        };
        self.shared.flags.recording.store(false, Relaxed);
        let _ = self.cmd_tx.send(Cmd::Recorder(None));
        handle.join().map_err(|_| "recorder panicked".to_string())??;
        Ok(Some(path))
    }

    pub fn is_recording(&self) -> bool {
        self.recorder.lock().unwrap().is_some()
    }

    // ------------------------------------------------------- voice lab --

    pub fn lab_record(&self) -> Result<(), String> {
        if !self.shared.input_ok.load(Relaxed) {
            return Err("No microphone is connected. Pick one in Audio setup.".into());
        }
        self.lab_stop();
        let cap = self.sample_rate() as usize * LAB_MAX_SECONDS;
        self.shared.lab_pos.store(0, Relaxed);
        self.shared.lab_len.store(cap as u64, Relaxed);
        self.shared.lab_state.store(LAB_RECORDING, Relaxed);
        self.lab_tx.send(LabCmd::Record(Vec::with_capacity(cap))).map_err(|e| e.to_string())
    }

    /// Stop recording; returns the take length in seconds.
    pub fn lab_stop_record(&self) -> Result<f64, String> {
        // Drop any take left over from an earlier, interrupted recording.
        while self.take_rx.try_recv().is_ok() {}
        self.lab_tx.send(LabCmd::StopRecord).map_err(|e| e.to_string())?;
        let take = self
            .take_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "The microphone stopped responding".to_string())?;
        self.shared.lab_state.store(LAB_IDLE, Relaxed);
        let secs = take.len() as f64 / self.sample_rate() as f64;
        self.shared.lab_len.store(take.len() as u64, Relaxed);
        self.shared.lab_pos.store(0, Relaxed);
        *self.take.lock().unwrap() = (!take.is_empty()).then(|| Arc::new(take));
        Ok(secs)
    }

    pub fn lab_play(&self, looped: bool) -> Result<(), String> {
        let take = self.take.lock().unwrap().clone().ok_or("Record a take first")?;
        if !self.shared.input_ok.load(Relaxed) {
            return Err("Playback runs through the microphone stream; connect a microphone.".into());
        }
        self.shared.lab_len.store(take.len() as u64, Relaxed);
        self.shared.lab_state.store(LAB_PLAYING, Relaxed);
        self.lab_tx.send(LabCmd::Play(take, looped)).map_err(|e| e.to_string())
    }

    pub fn lab_stop(&self) {
        match self.shared.lab_state.load(Relaxed) {
            LAB_RECORDING => {
                let _ = self.lab_stop_record();
            }
            LAB_PLAYING => {
                let _ = self.lab_tx.send(LabCmd::StopPlay);
                self.shared.lab_state.store(LAB_IDLE, Relaxed);
            }
            _ => {}
        }
    }

    pub fn lab_take(&self) -> Option<Arc<Vec<f32>>> {
        self.take.lock().unwrap().clone()
    }

    fn build_streams(&self, choice: &DeviceChoice) -> Result<((cpal::Stream, Option<cpal::Stream>), AudioStatus), String> {
        let host = cpal::default_host();
        let find = |name: &Option<String>, input: bool| -> Option<cpal::Device> {
            let name = name.as_ref()?;
            let mut it = if input { host.input_devices().ok()?.collect::<Vec<_>>() } else { host.output_devices().ok()?.collect::<Vec<_>>() }.into_iter();
            it.find(|d| &device_name(d) == name)
        };
        let out_dev = find(&choice.output, false)
            .or_else(|| host.default_output_device())
            .ok_or("No audio output device found")?;
        let out_default = out_dev.default_output_config().map_err(|e| format!("Output device: {e}"))?;
        let sr = out_default.sample_rate();
        let out_channels = out_default.channels() as usize;
        let buffer = choice.buffer.or(Some(256)).and_then(|b| match out_default.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } if b >= *min && b <= *max => Some(b),
            _ => None,
        });
        let out_cfg = cpal::StreamConfig {
            channels: out_default.channels(),
            sample_rate: sr,
            buffer_size: buffer.map(cpal::BufferSize::Fixed).unwrap_or_default(),
        };
        self.shared.sample_rate.store(sr, Relaxed);

        let vocal_rb = HeapRb::<f32>::new(sr as usize); // 0.5 s stereo
        let (vocal_prod, vocal_cons) = vocal_rb.split();
        let frame_rb = HeapRb::<StampedFrame>::new(4096);
        let (frame_prod, frame_cons) = frame_rb.split();
        *self.frames.lock().unwrap() = Some(frame_cons);

        let output = OutputState {
            shared: self.shared.clone(),
            cmd_rx: self.cmd_rx.clone(),
            garbage: self.garbage_tx.clone(),
            track: None,
            pos: self.shared.position.load(Relaxed),
            vocal: vocal_cons,
            recorder: None,
            mix: Vec::with_capacity(16_384),
            vocal_tmp: vec![0.0; 16_384],
            rec_tmp: Vec::with_capacity(16_384),
            limiter: Limiter::new(sr as f32),
            drone: Drone::new(sr),
            channels: out_channels,
            sr,
        };
        let shared = self.shared.clone();
        let err = move |e: cpal::Error| {
            *shared.stream_error.lock().unwrap() = Some(e.to_string());
        };
        let out_stream = match out_default.sample_format() {
            SampleFormat::F32 => build_output::<f32>(&out_dev, out_cfg, output, err),
            SampleFormat::I16 => build_output::<i16>(&out_dev, out_cfg, output, err),
            SampleFormat::I32 => build_output::<i32>(&out_dev, out_cfg, output, err),
            SampleFormat::U16 => build_output::<u16>(&out_dev, out_cfg, output, err),
            f => Err(format!("Unsupported output sample format {f}")),
        }?;
        out_stream.play().map_err(|e| e.to_string())?;

        let mut status = AudioStatus {
            sample_rate: sr,
            output: device_name(&out_dev),
            buffer,
            ..Default::default()
        };

        let in_dev = find(&choice.input, true).or_else(|| host.default_input_device());
        let in_stream = match in_dev {
            None => {
                status.input_error = Some("No microphone found".into());
                None
            }
            Some(dev) => {
                status.input = Some(device_name(&dev));
                match self.build_input(&dev, sr, buffer, vocal_prod, frame_prod) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        status.input_error = Some(e);
                        None
                    }
                }
            }
        };
        self.shared.input_ok.store(in_stream.is_some(), Relaxed);
        status.latency_ms = self.shared.chain_latency.load(Relaxed) as f32 / sr as f32 * 1000.0
            + 2.0 * buffer.unwrap_or(512) as f32 / sr as f32 * 1000.0;
        Ok(((out_stream, in_stream), status))
    }

    fn build_input(
        &self,
        dev: &cpal::Device,
        sr: u32,
        buffer: Option<u32>,
        vocal: HeapProd<f32>,
        frames: HeapProd<StampedFrame>,
    ) -> Result<cpal::Stream, String> {
        // Prefer a config at the output rate so no resampling is needed.
        let same_rate = dev.supported_input_configs().ok().and_then(|cfgs| {
            let mut v: Vec<_> = cfgs.filter(|c| c.min_sample_rate() <= sr && c.max_sample_rate() >= sr).collect();
            v.sort_by_key(|c| (c.sample_format() != SampleFormat::F32, c.channels()));
            v.into_iter().next().map(|c| c.with_sample_rate(sr))
        });
        let supported = match same_rate {
            Some(c) => c,
            None => dev.default_input_config().map_err(|e| format!("Microphone: {e}"))?,
        };
        let in_sr = supported.sample_rate();
        let buffer = buffer.and_then(|b| match supported.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } if b >= *min && b <= *max => Some(b),
            _ => None,
        });
        let cfg = cpal::StreamConfig {
            channels: supported.channels(),
            sample_rate: in_sr,
            buffer_size: buffer.map(cpal::BufferSize::Fixed).unwrap_or_default(),
        };
        let chain = VocalChain::new(sr as f32);
        self.shared.chain_latency.store(chain.latency() as u32, Relaxed);
        let state = InputState {
            shared: self.shared.clone(),
            chain,
            vocal,
            frames,
            channels: supported.channels() as usize,
            mono: Vec::with_capacity(16_384),
            resampled: Vec::with_capacity(16_384),
            stereo: Vec::with_capacity(32_768),
            resampler: (in_sr != sr).then(|| LinearResampler::new(in_sr as f64 / sr as f64)),
            lab_rx: self.lab_rx.clone(),
            take_tx: self.take_tx.clone(),
            garbage: self.buf_garbage_tx.clone(),
            take: None,
            play: None,
        };
        let shared = self.shared.clone();
        let err = move |e: cpal::Error| {
            *shared.stream_error.lock().unwrap() = Some(format!("Microphone: {e}"));
        };
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build_input::<f32>(dev, cfg, state, err),
            SampleFormat::I16 => build_input::<i16>(dev, cfg, state, err),
            SampleFormat::I32 => build_input::<i32>(dev, cfg, state, err),
            SampleFormat::U16 => build_input::<u16>(dev, cfg, state, err),
            f => Err(format!("Unsupported microphone sample format {f}")),
        }?;
        stream.play().map_err(|e| format!("Microphone: {e}"))?;
        Ok(stream)
    }
}

// ------------------------------------------------------------- output --

struct OutputState {
    shared: Arc<Shared>,
    cmd_rx: Receiver<Cmd>,
    garbage: Sender<Arc<Track>>,
    track: Option<Arc<Track>>,
    pos: u64,
    vocal: HeapCons<f32>,
    recorder: Option<HeapProd<f32>>,
    mix: Vec<f32>,
    vocal_tmp: Vec<f32>,
    rec_tmp: Vec<f32>,
    limiter: Limiter,
    drone: Drone,
    channels: usize,
    sr: u32,
}

/// A soft sustained chord on the current key, to sing against in the Voice Lab.
struct Drone {
    phases: [f64; 8],
    incs: [f64; 8],
    gain: f32,
    key: (i32, usize),
    sr: f64,
}

impl Drone {
    fn new(sr: u32) -> Self {
        Self { phases: [0.0; 8], incs: [0.0; 8], gain: 0.0, key: (-1, 0), sr: sr as f64 }
    }

    fn retune(&mut self, tonic: i32, scale_idx: usize) {
        if self.key == (tonic, scale_idx) {
            return;
        }
        self.key = (tonic, scale_idx);
        let sc = super::scale::scale(scale_idx);
        let third = if sc.contains(&4) { 4 } else { 3 };
        let root = 48 + tonic.rem_euclid(12);
        // Root, fifth, octave and third; each voice doubled with a slight detune.
        let notes = [root, root + 7, root + 12, root + 12 + third];
        for (k, n) in notes.iter().enumerate() {
            let f = 440.0 * 2f64.powf((*n as f64 - 69.0) / 12.0);
            self.incs[k * 2] = f * 2f64.powf(-0.04 / 12.0) / self.sr;
            self.incs[k * 2 + 1] = f * 2f64.powf(0.04 / 12.0) / self.sr;
        }
    }

    #[inline]
    fn next(&mut self, level: f32) -> (f32, f32) {
        self.gain += (level * 0.09 - self.gain) * 0.0004;
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for k in 0..8 {
            self.phases[k] = (self.phases[k] + self.incs[k]).fract();
            let ph = self.phases[k] * std::f64::consts::TAU;
            let v = (ph.sin() + 0.25 * (2.0 * ph).sin()) as f32;
            if k % 2 == 0 {
                l += v;
            } else {
                r += v;
            }
        }
        (l * self.gain, r * self.gain)
    }
}

impl OutputState {
    fn handle_commands(&mut self) {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match cmd {
                Cmd::SetTrack(t, pos) => {
                    if let Some(old) = std::mem::replace(&mut self.track, t) {
                        let _ = self.garbage.try_send(old);
                    }
                    self.pos = pos;
                }
                Cmd::Seek(pos) => self.pos = pos,
                Cmd::Recorder(r) => self.recorder = r,
            }
        }
    }

    #[inline]
    fn frame_at(buf: &[f32], frame: i64) -> (f32, f32) {
        if frame < 0 {
            return (0.0, 0.0);
        }
        let i = frame as usize * 2;
        if i + 1 < buf.len() {
            (buf[i], buf[i + 1])
        } else {
            (0.0, 0.0)
        }
    }

    /// Render `frames` frames of interleaved stereo into `self.mix`.
    fn render(&mut self, frames: usize, out_latency: f32) {
        self.handle_commands();
        let p = &self.shared.params;
        let playing = self.shared.flags.playing.load(Relaxed);
        // Voice Lab playback must be audible even with monitoring off.
        let lab_playing = self.shared.lab_state.load(Relaxed) == LAB_PLAYING;
        let (master, inst, guide, vocal, monitor) =
            (p.master.get(), p.instrumental.get(), p.guide.get(), p.vocal.get(), p.monitor.get() > 0.5 || lab_playing);
        let drone_level = p.drone.get().clamp(0.0, 1.0);
        let drone_on = drone_level > 0.0 || self.drone.gain > 1e-4;
        if drone_on {
            self.drone.retune(p.tune_key.get().round() as i32, p.tune_scale.get().round() as usize);
        }

        // Keep the mic → speaker path tight: input and output clocks drift,
        // so drop backlog beyond what this callback needs plus a margin.
        let backlog = self.vocal.occupied_len() / 2;
        let keep = frames + 128;
        if backlog > keep + 256 {
            self.vocal.skip((backlog - keep) * 2);
        }
        if self.vocal_tmp.len() < frames * 2 {
            self.vocal_tmp.resize(frames * 2, 0.0);
        }
        let got = self.vocal.pop_slice(&mut self.vocal_tmp[..frames * 2]);
        self.vocal_tmp[got..frames * 2].fill(0.0);

        let recording = self.recorder.is_some();
        // Line the recorded vocal up with what the singer heard.
        let comp = if recording {
            let manual = p.record_latency_ms.get();
            let secs = if manual >= 0.0 {
                manual / 1000.0
            } else {
                out_latency
                    + self.shared.in_latency.get()
                    + (self.shared.chain_latency.load(Relaxed) as usize + backlog.min(keep)) as f32 / self.sr as f32
            };
            (secs * self.sr as f32) as i64
        } else {
            0
        };

        self.mix.clear();
        self.rec_tmp.clear();
        let len = self.track.as_ref().map(|t| t.frames()).unwrap_or(0);
        // Snapshot what is playable for this whole block (one atomic read).
        let (audio, guide_buf, guide_offset): (&[f32], &[f32], i64) = match &self.track {
            Some(t) => (t.audio.ready(), t.guide.as_ref().map(|g| g.ready()).unwrap_or(&[]), t.guide_offset),
            None => (&[], &[], 0),
        };
        let ready = (audio.len() / 2) as u64;
        let mut buffering = false;
        let mut peak = 0.0f32;
        for i in 0..frames {
            let (mut bl, mut br) = (0.0, 0.0);
            let (mut rl, mut rr) = (0.0, 0.0);
            if self.track.is_some() && playing && self.pos < len {
                if self.pos >= ready {
                    // Hold position until the separator catches up.
                    buffering = true;
                } else {
                    let f = self.pos as i64;
                    let (il, ir) = Self::frame_at(audio, f);
                    bl = il * inst;
                    br = ir * inst;
                    if !guide_buf.is_empty() {
                        let (gl, gr) = Self::frame_at(guide_buf, f - guide_offset);
                        bl += gl * guide;
                        br += gr * guide;
                    }
                    if recording {
                        let (il, ir) = Self::frame_at(audio, f - comp);
                        rl = il * inst;
                        rr = ir * inst;
                    }
                    self.pos += 1;
                }
            }
            let vl = self.vocal_tmp[i * 2] * vocal;
            let vr = self.vocal_tmp[i * 2 + 1] * vocal;
            if drone_on {
                let d = self.drone.next(drone_level);
                bl += d.0;
                br += d.1;
            }
            let (l, r) = if monitor { (bl + vl, br + vr) } else { (bl, br) };
            let (l, r) = self.limiter.run(l * master, r * master);
            peak = peak.max(l.abs()).max(r.abs());
            self.mix.push(l);
            self.mix.push(r);
            if recording {
                self.rec_tmp.push(((rl + vl) * master).clamp(-1.0, 1.0));
                self.rec_tmp.push(((rr + vr) * master).clamp(-1.0, 1.0));
            }
        }
        if let Some(rec) = &mut self.recorder {
            if playing && !buffering {
                rec.push_slice(&self.rec_tmp);
            }
        }
        self.shared.buffering.store(buffering, Relaxed);
        if playing && self.track.is_some() && self.pos >= len {
            self.shared.flags.playing.store(false, Relaxed);
            self.shared.ended.store(true, Relaxed);
        }
        self.shared.position.store(self.pos, Relaxed);
        let prev = self.shared.out_peak.get();
        self.shared.out_peak.set(peak.max(prev * 0.9));
    }
}

fn build_output<T>(
    dev: &cpal::Device,
    cfg: cpal::StreamConfig,
    mut state: OutputState,
    err: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let shared = state.shared.clone();
    dev.build_output_stream::<T, _, _>(
        cfg,
        move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
            let ts = info.timestamp();
            let lat = ts.playback.duration_since(ts.callback).as_secs_f32();
            shared.out_latency.set(lat);
            let ch = state.channels;
            let frames = data.len() / ch;
            state.render(frames, lat);
            for (f, out) in data.chunks_mut(ch).enumerate() {
                let (l, r) = (state.mix[f * 2], state.mix[f * 2 + 1]);
                if ch == 1 {
                    out[0] = T::from_sample((l + r) * 0.5);
                } else {
                    out[0] = T::from_sample(l);
                    out[1] = T::from_sample(r);
                    for o in out.iter_mut().skip(2) {
                        *o = T::from_sample(0.0f32);
                    }
                }
            }
        },
        err,
        None,
    )
    .map_err(|e| format!("Output device: {e}"))
}

// -------------------------------------------------------------- input --

struct LinearResampler {
    step: f64,
    pos: f64,
    last: f32,
}

impl LinearResampler {
    fn new(step: f64) -> Self {
        Self { step, pos: 0.0, last: 0.0 }
    }

    /// `x[0]` is the previous block's last sample, `x[k]` is `input[k-1]`.
    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        out.clear();
        let n = input.len() as f64;
        while self.pos < n {
            let i = self.pos.floor() as usize;
            let f = (self.pos - i as f64) as f32;
            let a = if i == 0 { self.last } else { input[i - 1] };
            let b = input[i];
            out.push(a + (b - a) * f);
            self.pos += self.step;
        }
        self.pos -= n;
        if let Some(&l) = input.last() {
            self.last = l;
        }
    }
}

struct InputState {
    shared: Arc<Shared>,
    chain: VocalChain,
    vocal: HeapProd<f32>,
    frames: HeapProd<StampedFrame>,
    channels: usize,
    mono: Vec<f32>,
    resampled: Vec<f32>,
    stereo: Vec<f32>,
    resampler: Option<LinearResampler>,
    lab_rx: Receiver<LabCmd>,
    take_tx: Sender<Vec<f32>>,
    garbage: Sender<Arc<Vec<f32>>>,
    take: Option<Vec<f32>>,
    play: Option<(Arc<Vec<f32>>, usize, bool)>,
}

impl InputState {
    fn lab_commands(&mut self) {
        while let Ok(cmd) = self.lab_rx.try_recv() {
            match cmd {
                LabCmd::Record(buf) => self.take = Some(buf),
                LabCmd::StopRecord => {
                    let buf = self.take.take().unwrap_or_default();
                    let _ = self.take_tx.try_send(buf);
                }
                LabCmd::Play(buf, looped) => {
                    if let Some((old, _, _)) = self.play.replace((buf, 0, looped)) {
                        let _ = self.garbage.try_send(old);
                    }
                }
                LabCmd::StopPlay => {
                    if let Some((old, _, _)) = self.play.take() {
                        let _ = self.garbage.try_send(old);
                    }
                }
            }
        }
    }

    fn consume(&mut self) {
        self.lab_commands();
        if let Some(rs) = &mut self.resampler {
            rs.process(&self.mono, &mut self.resampled);
            std::mem::swap(&mut self.mono, &mut self.resampled);
        }
        let peak = self.mono.iter().fold(0.0f32, |a, &x| a.max(x.abs()));
        let prev = self.shared.mic_peak.get();
        self.shared.mic_peak.set(peak.max(prev * 0.9));

        // Voice Lab: capture the raw mic (within the pre-allocated capacity).
        if let Some(take) = &mut self.take {
            let room = take.capacity() - take.len();
            take.extend_from_slice(&self.mono[..self.mono.len().min(room)]);
            self.shared.lab_pos.store(take.len() as u64, Relaxed);
        }
        // Voice Lab: replace the mic with the recorded take.
        let mut finished = false;
        if let Some((buf, pos, looped)) = &mut self.play {
            for s in self.mono.iter_mut() {
                if *pos >= buf.len() {
                    if *looped && !buf.is_empty() {
                        *pos = 0;
                    } else {
                        *s = 0.0;
                        finished = true;
                        continue;
                    }
                }
                *s = buf[*pos];
                *pos += 1;
            }
            self.shared.lab_pos.store(*pos as u64, Relaxed);
        }
        if finished {
            if let Some((old, _, _)) = self.play.take() {
                let _ = self.garbage.try_send(old);
            }
            self.shared.lab_state.store(LAB_IDLE, Relaxed);
        }

        let pos = self.shared.position.load(Relaxed);
        let frames = &mut self.frames;
        self.chain.process(&self.mono, &mut self.stereo, &self.shared.params, |frame| {
            let _ = frames.try_push(StampedFrame { frame, pos });
        });
        if self.play.is_some() && self.shared.lab_bypass.load(Relaxed) {
            for (o, &x) in self.stereo.chunks_exact_mut(2).zip(&self.mono) {
                o[0] = x * 0.707;
                o[1] = x * 0.707;
            }
        }
        self.vocal.push_slice(&self.stereo);
    }
}

fn build_input<T>(
    dev: &cpal::Device,
    cfg: cpal::StreamConfig,
    mut state: InputState,
    err: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let shared = state.shared.clone();
    dev.build_input_stream::<T, _, _>(
        cfg,
        move |data: &[T], info: &cpal::InputCallbackInfo| {
            let ts = info.timestamp();
            shared.in_latency.set(ts.callback.duration_since(ts.capture).as_secs_f32());
            let ch = state.channels.max(1);
            state.mono.clear();
            // Sum rather than average: a mic on one channel of a stereo
            // interface would otherwise lose 6 dB.
            for frame in data.chunks(ch) {
                let s: f32 = frame.iter().map(|&x| x.to_sample::<f32>()).sum();
                state.mono.push(s.clamp(-1.0, 1.0));
            }
            state.consume();
        },
        err,
        None,
    )
    .map_err(|e| format!("Microphone: {e}"))
}
