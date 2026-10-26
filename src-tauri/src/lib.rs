pub mod audio;
mod meta;
mod tools;
mod youtube;

use audio::analysis::{self, KeyEstimate};
use audio::engine::{self, AudioStatus, Buf, DeviceChoice, DeviceList, Engine, Track};
use audio::separate::{Growing, GrowingWriter, Separator, MODEL, MODEL_RATE};
use ringbuf::traits::Consumer;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

struct Dirs {
    tools: PathBuf,
    cache: PathBuf,
    /// Separated instrumental/vocal stems, so repeat songs start instantly.
    stems: PathBuf,
    recordings: PathBuf,
}

#[derive(Default)]
struct Song {
    /// The original recording the song is built from.
    source: Option<PathBuf>,
    /// YouTube id of `source`; names its cached stems.
    source_id: Option<String>,
    /// Decoded, untransformed audio at `sr` (None while separating).
    inst: Option<Arc<Vec<f32>>>,
    /// The isolated original vocal, used as a guide.
    guide: Option<Arc<Vec<f32>>>,
    key: Option<KeyEstimate>,
    semitones: f32,
    tempo: f32,
    sr: u32,
    vocal_removed: bool,
    /// Length in frames while the separator is still running.
    separating: Option<u64>,
    /// The UI's id for the separation that built this song.
    job: u32,
}

struct AppState {
    engine: Arc<Engine>,
    dirs: Dirs,
    song: Mutex<Song>,
    /// Bumped whenever the loaded audio changes; stale background work is dropped.
    generation: Arc<AtomicU32>,
    /// The AI model, loaded once and reused for every song.
    separator: Arc<Mutex<Option<Separator>>>,
    /// The running separation. Only one runs at a time, so two copies of the
    /// model never compete for memory.
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

type Res<T> = Result<T, String>;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SongInfo {
    duration: f64,
    source_duration: f64,
    key: Option<KeyEstimate>,
    semitones: f32,
    tempo: f32,
    has_guide: bool,
    separating: bool,
    vocal_removed: bool,
}

fn info(song: &Song) -> SongInfo {
    let frames = song
        .inst
        .as_ref()
        .map(|a| (a.len() / 2) as u64)
        .or(song.separating)
        .unwrap_or(0) as f64;
    let sr = song.sr.max(1) as f64;
    SongInfo {
        duration: frames / sr / song.tempo as f64,
        source_duration: frames / sr,
        key: song.key,
        semitones: song.semitones,
        tempo: song.tempo,
        has_guide: song.guide.is_some() || song.separating.is_some(),
        separating: song.separating.is_some(),
        vocal_removed: song.vocal_removed,
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Res<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())
}

/// Render the current song with its key/tempo settings and hand it to the
/// engine, keeping the musical position.
async fn apply_track(state: &AppState) -> Res<SongInfo> {
    let gen = state.generation.load(Relaxed);
    let (inst, guide, semis, tempo, sr) = {
        let s = state.song.lock().unwrap();
        if s.separating.is_some() {
            return Err("Key and tempo changes unlock once the vocals are separated.".into());
        }
        let Some(inst) = s.inst.clone() else { return Err("No song loaded".into()) };
        (inst, s.guide.clone(), s.semitones, s.tempo, s.sr)
    };
    let old_tempo = {
        let t = state.engine.current_track();
        let frames = t.map(|t| t.frames()).unwrap_or(0) as f64;
        if frames > 0.0 {
            (inst.len() / 2) as f64 / frames
        } else {
            1.0
        }
    };
    let source_pos = state.engine.shared.position.load(Relaxed) as f64 * old_tempo;

    let (audio, guide) = blocking(move || {
        std::thread::scope(|sc| {
            let g = guide.as_ref().map(|g| sc.spawn(|| audio::stretch::render(g, sr, semis, tempo)));
            let a = audio::stretch::render(&inst, sr, semis, tempo);
            (a, g.map(|h| h.join().unwrap()))
        })
    })
    .await?;
    if state.generation.load(Relaxed) != gen {
        return Err("superseded".into());
    }
    let track = Track {
        audio: Buf::Full(Arc::new(audio)),
        guide: guide.map(|g| Buf::Full(Arc::new(g))),
        guide_offset: 0,
    };
    let pos = (source_pos / tempo as f64) as u64;
    state.engine.set_track(Some(Arc::new(track)), pos);
    Ok(info(&state.song.lock().unwrap()))
}

// ---------------------------------------------------------------- setup --

#[tauri::command]
fn tools_status(state: State<'_, AppState>) -> tools::ToolStatus {
    tools::status(&state.dirs.tools)
}

#[tauri::command]
async fn tools_install(app: AppHandle, state: State<'_, AppState>) -> Res<tools::ToolStatus> {
    tools::install(&app, &state.dirs.tools).await
}

// ------------------------------------------------------------- catalogue --

#[tauri::command]
async fn search_songs(query: String) -> Res<Vec<meta::SongMeta>> {
    meta::search_songs(query.trim()).await
}

#[tauri::command]
async fn get_lyrics(artist: String, title: String, album: String, duration: f64) -> Res<meta::Lyrics> {
    meta::lyrics(&artist, &title, &album, duration).await
}

#[tauri::command]
async fn yt_search(state: State<'_, AppState>, query: String, limit: u32) -> Res<Vec<youtube::Video>> {
    youtube::search(&state.dirs.tools, &query, limit).await
}

#[tauri::command]
async fn yt_download(app: AppHandle, state: State<'_, AppState>, id: String) -> Res<String> {
    youtube::download(&app, &state.dirs.tools, &state.dirs.cache, &id)
        .await
        .map(|p| p.to_string_lossy().into_owned())
}

// ------------------------------------------------------------------ song --

fn stem_paths(dir: &Path, id: &str) -> Res<(PathBuf, PathBuf)> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("Invalid source id".into());
    }
    Ok((dir.join(format!("{id}.instrumental.wav")), dir.join(format!("{id}.vocals.wav"))))
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SeparateProgress {
    /// The caller's job id; events for any other job are stale.
    job: u32,
    fraction: f64,
    /// Seconds of the song that are ready to play.
    ready: f64,
    /// Separation speed relative to real time.
    speed: f64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SeparateDone {
    job: u32,
    info: SongInfo,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SeparateError {
    job: u32,
    message: String,
}

/// Build the karaoke track from the original recording: the AI model splits
/// it into instrumental and vocals. Returns as soon as playback can begin;
/// the rest is separated in the background ("separate://progress", then
/// "separate://done" or "separate://error"). Results are cached per source.
#[tauri::command]
async fn song_separate(app: AppHandle, state: State<'_, AppState>, path: String, id: String, job: u32) -> Res<SongInfo> {
    state.engine.pause();
    let gen = state.generation.fetch_add(1, Relaxed) + 1;
    // The bump above tells a running separation to stop; wait for it (at
    // most one model chunk) so the model is free and memory isn't doubled.
    let previous = state.worker.lock().unwrap().take();
    if let Some(handle) = previous {
        let _ = blocking(move || handle.join()).await;
    }
    let sr = state.engine.sample_rate();
    let (inst_wav, vox_wav) = stem_paths(&state.dirs.stems, &id)?;
    let fresh = |s: &AppState| s.generation.load(Relaxed) == gen;

    if inst_wav.is_file() && vox_wav.is_file() {
        let (inst, vox, key) = blocking(move || -> Res<_> {
            let inst = audio::decode::decode_file(&inst_wav, sr)?;
            let vox = audio::decode::decode_file(&vox_wav, sr)?;
            let full: Vec<f32> = inst.iter().zip(&vox).map(|(a, b)| a + b).collect();
            let key = analysis::detect_key(&analysis::mono(&full), sr);
            Ok((inst, vox, key))
        })
        .await??;
        if !fresh(&state) {
            return Err("superseded".into());
        }
        *state.song.lock().unwrap() = Song {
            source: Some(path.into()),
            source_id: Some(id),
            job,
            inst: Some(Arc::new(inst)),
            guide: Some(Arc::new(vox)),
            key: Some(key),
            semitones: 0.0,
            tempo: 1.0,
            sr,
            ..Default::default()
        };
        state.engine.set_track(None, 0);
        return apply_track(&state).await;
    }

    let model = tools::ensure_model(&app, &state.dirs.tools).await?;
    let p = PathBuf::from(&path);
    let (mix, key) = blocking(move || -> Res<_> {
        let mix = audio::decode::decode_file(&p, MODEL_RATE)?;
        let key = analysis::detect_key(&analysis::mono(&mix), MODEL_RATE);
        Ok((mix, key))
    })
    .await??;
    if !fresh(&state) {
        return Err("superseded".into());
    }

    let frames = ((mix.len() / 2) as f64 * sr as f64 / MODEL_RATE as f64).round() as usize;
    let (inst_buf, inst_w) = Growing::new(frames * 2);
    let (vox_buf, vox_w) = Growing::new(frames * 2);
    let info_now = {
        let mut s = state.song.lock().unwrap();
        *s = Song {
            source: Some(path.into()),
            source_id: Some(id),
            job,
            key: Some(key),
            semitones: 0.0,
            tempo: 1.0,
            sr,
            separating: Some(frames as u64),
            ..Default::default()
        };
        info(&s)
    };
    let track = Track { audio: Buf::Growing(inst_buf.clone()), guide: Some(Buf::Growing(vox_buf.clone())), guide_offset: 0 };
    state.engine.set_track(Some(Arc::new(track)), 0);

    let work = SeparationJob {
        app: app.clone(),
        job,
        separator: state.separator.clone(),
        model,
        mix,
        sr,
        gen,
        inst_w,
        vox_w,
        inst_buf,
        vox_buf,
        inst_wav,
        vox_wav,
    };
    let handle = std::thread::Builder::new()
        .name("vocal-separation".into())
        .spawn(move || work.run())
        .map_err(|e| e.to_string())?;
    *state.worker.lock().unwrap() = Some(handle);
    Ok(info_now)
}

struct SeparationJob {
    app: AppHandle,
    job: u32,
    separator: Arc<Mutex<Option<Separator>>>,
    model: PathBuf,
    /// Interleaved stereo at [`MODEL_RATE`].
    mix: Vec<f32>,
    sr: u32,
    gen: u32,
    inst_w: GrowingWriter,
    vox_w: GrowingWriter,
    inst_buf: Arc<Growing>,
    vox_buf: Arc<Growing>,
    inst_wav: PathBuf,
    vox_wav: PathBuf,
}

impl SeparationJob {
    fn run(self) {
        let app = self.app.clone();
        let (gen, job) = (self.gen, self.job);
        let (inst_buf, vox_buf) = (self.inst_buf.clone(), self.vox_buf.clone());
        let result = self.separate();
        let state = app.state::<AppState>();
        if state.generation.load(Relaxed) != gen {
            // Superseded: another song took over; it owns the UI now.
            return;
        }
        let result = result.and_then(|()| {
            // Swap the growing buffers for regular ones so key/tempo changes
            // work; same samples, so playback doesn't skip.
            {
                let mut s = state.song.lock().unwrap();
                s.inst = Some(Arc::new(inst_buf.ready_slice().to_vec()));
                s.guide = Some(Arc::new(vox_buf.ready_slice().to_vec()));
                s.separating = None;
            }
            tauri::async_runtime::block_on(apply_track(&state))
        });
        match result {
            Ok(info) => {
                let _ = app.emit("separate://done", SeparateDone { job, info });
            }
            Err(e) if e == "cancelled" || e == "superseded" => {}
            Err(message) => {
                let _ = app.emit("separate://error", SeparateError { job, message });
            }
        }
    }

    fn separate(self) -> Res<()> {
        let Self { app, job, separator, model, mix, sr, gen, mut inst_w, mut vox_w, inst_wav, vox_wav, .. } = self;
        let state = app.state::<AppState>();
        let cached = separator.lock().unwrap().take();
        let mut sep = match cached {
            Some(sep) => sep,
            None => Separator::load(&model, MODEL.config)?,
        };
        let result = Self::stream(&app, &state, &mut sep, job, gen, &mix, sr, &mut inst_w, &mut vox_w);
        // Keep the model loaded for the next song, whatever happened.
        *separator.lock().unwrap() = Some(sep);
        let (inst_all, vox_all) = result?;
        inst_w.finish();
        vox_w.finish();
        // A failed cache write only costs time next play.
        let _ = audio::decode::write_wav(&inst_wav, &inst_all, MODEL_RATE)
            .and_then(|_| audio::decode::write_wav(&vox_wav, &vox_all, MODEL_RATE));
        Ok(())
    }

    /// Run the model over the song, streaming results into the playback
    /// buffers. Returns the full-rate stems for the cache.
    #[allow(clippy::too_many_arguments)]
    fn stream(
        app: &AppHandle,
        state: &AppState,
        sep: &mut Separator,
        job: u32,
        gen: u32,
        mix: &[f32],
        sr: u32,
        inst_w: &mut GrowingWriter,
        vox_w: &mut GrowingWriter,
    ) -> Res<(Vec<f32>, Vec<f32>)> {
        let mut rs_inst = audio::decode::StreamResampler::new(MODEL_RATE, sr)?;
        let mut rs_vox = audio::decode::StreamResampler::new(MODEL_RATE, sr)?;
        let mut inst_all = Vec::with_capacity(mix.len());
        let mut vox_all = Vec::with_capacity(mix.len());
        let (mut out, mut vox) = (Vec::new(), Vec::new());
        let started = Instant::now();
        let mut failure: Option<String> = None;
        sep.run(mix, |seg, fraction| {
            if state.generation.load(Relaxed) != gen {
                return false;
            }
            let base = inst_all.len();
            vox.clear();
            vox.extend(seg.iter().enumerate().map(|(i, &v)| mix[base + i] - v));
            inst_all.extend_from_slice(seg);
            vox_all.extend_from_slice(&vox);
            for (rs, src, w) in [(&mut rs_inst, seg, &mut *inst_w), (&mut rs_vox, &vox[..], &mut *vox_w)] {
                if let Err(e) = rs.push(src, &mut out) {
                    failure = Some(e);
                    return false;
                }
                w.append(&out);
            }
            let ready = inst_all.len() as f64 / 2.0 / MODEL_RATE as f64;
            let speed = ready / started.elapsed().as_secs_f64().max(1e-3);
            let _ = app.emit("separate://progress", SeparateProgress { job, fraction: fraction as f64, ready, speed });
            true
        })
        .map_err(|e| failure.clone().unwrap_or(e))?;
        rs_inst.flush(&mut out)?;
        inst_w.append(&out);
        rs_vox.flush(&mut out)?;
        vox_w.append(&out);
        Ok((inst_all, vox_all))
    }
}

/// Which of these sources already have separated stems on disk (their
/// songs start instantly).
#[tauri::command]
fn stems_cached(state: State<'_, AppState>, ids: Vec<String>) -> Vec<String> {
    ids.into_iter()
        .filter(|id| stem_paths(&state.dirs.stems, id).map(|(i, v)| i.is_file() && v.is_file()).unwrap_or(false))
        .collect()
}

/// Fallback when the AI model can't run: a basic centre-channel remover.
#[tauri::command]
async fn song_vocal_removal(state: State<'_, AppState>, path: String) -> Res<SongInfo> {
    state.engine.pause();
    let gen = state.generation.fetch_add(1, Relaxed) + 1;
    let sr = state.engine.sample_rate();
    let p = PathBuf::from(&path);
    let (inst, guide, key) = blocking(move || -> Res<_> {
        let guide = audio::decode::decode_file(&p, sr)?;
        let inst = analysis::remove_center(&guide, sr);
        let key = analysis::detect_key(&analysis::mono(&guide), sr);
        Ok((inst, guide, key))
    })
    .await??;
    if state.generation.load(Relaxed) != gen {
        return Err("superseded".into());
    }
    *state.song.lock().unwrap() = Song {
        source: Some(path.into()),
        inst: Some(Arc::new(inst)),
        guide: Some(Arc::new(guide)),
        key: Some(key),
        semitones: 0.0,
        tempo: 1.0,
        sr,
        vocal_removed: true,
        ..Default::default()
    };
    state.engine.set_track(None, 0);
    apply_track(&state).await
}

#[tauri::command]
async fn song_transform(state: State<'_, AppState>, semitones: f32, tempo: f32) -> Res<SongInfo> {
    {
        let mut s = state.song.lock().unwrap();
        if s.separating.is_some() {
            return Err("Key and tempo changes unlock once the vocals are separated.".into());
        }
        s.semitones = semitones.clamp(-12.0, 12.0);
        s.tempo = tempo.clamp(0.5, 1.5);
    }
    apply_track(&state).await
}

#[tauri::command]
fn song_unload(state: State<'_, AppState>) {
    state.generation.fetch_add(1, Relaxed);
    state.engine.pause();
    state.engine.set_track(None, 0);
    *state.song.lock().unwrap() = Song::default();
}

// ------------------------------------------------------------- transport --

#[tauri::command]
fn transport_play(state: State<'_, AppState>) {
    state.engine.play();
}

#[tauri::command]
fn transport_pause(state: State<'_, AppState>) {
    state.engine.pause();
}

#[tauri::command]
fn transport_seek(state: State<'_, AppState>, seconds: f64) {
    let sr = state.engine.sample_rate() as f64;
    state.engine.seek_frames((seconds.max(0.0) * sr) as u64);
}

#[tauri::command]
fn set_params(state: State<'_, AppState>, params: Vec<(String, f32)>) -> Vec<String> {
    params
        .into_iter()
        .filter(|(k, v)| !state.engine.shared.params.set(k, *v))
        .map(|(k, _)| k)
        .collect()
}

// ----------------------------------------------------------------- audio --

#[tauri::command]
fn audio_devices() -> DeviceList {
    engine::list_devices()
}

#[tauri::command]
async fn audio_open(
    app: AppHandle,
    state: State<'_, AppState>,
    input: Option<String>,
    output: Option<String>,
    buffer: Option<u32>,
) -> Res<AudioStatus> {
    let engine = state.engine.clone();
    let status = blocking(move || engine.open(DeviceChoice { input, output, buffer })).await??;
    // A new device may run at a different rate; rebuild the song if so.
    let reload = {
        let s = state.song.lock().unwrap();
        let loaded = s.inst.is_some() || s.separating.is_some();
        (loaded && s.sr != status.sample_rate).then(|| (s.source.clone(), s.source_id.clone(), s.vocal_removed, s.job))
    };
    if let Some((Some(source), id, removed, job)) = reload {
        let source = source.to_string_lossy().into_owned();
        match (removed, id) {
            (false, Some(id)) => {
                // Same job id, so the UI keeps following its progress.
                song_separate(app, state.clone(), source, id, job).await?;
            }
            _ => {
                song_vocal_removal(state.clone(), source).await?;
            }
        }
    }
    Ok(status)
}

#[tauri::command]
fn audio_status(state: State<'_, AppState>) -> AudioStatus {
    state.engine.status()
}

#[tauri::command]
fn record_start(state: State<'_, AppState>, name: String) -> Res<String> {
    let safe: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || " -_()".contains(c) { c } else { '_' })
        .collect::<String>()
        .trim()
        .chars()
        .take(100)
        .collect();
    let mut path = state.dirs.recordings.join(format!("{safe}.wav"));
    let mut n = 2;
    while path.exists() {
        path = state.dirs.recordings.join(format!("{safe} ({n}).wav"));
        n += 1;
    }
    state.engine.start_recording(path.clone())?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
fn record_stop(state: State<'_, AppState>) -> Res<Option<String>> {
    Ok(state.engine.stop_recording()?.map(|p| p.to_string_lossy().into_owned()))
}

// ------------------------------------------------------------- voice lab --

#[tauri::command]
fn lab_record(state: State<'_, AppState>) -> Res<()> {
    state.engine.lab_record()
}

#[tauri::command]
fn lab_stop_record(state: State<'_, AppState>) -> Res<f64> {
    state.engine.lab_stop_record()
}

#[tauri::command]
fn lab_play(state: State<'_, AppState>, looped: bool) -> Res<()> {
    state.engine.lab_play(looped)
}

#[tauri::command]
fn lab_stop(state: State<'_, AppState>) {
    state.engine.lab_stop();
}

#[tauri::command]
fn lab_bypass(state: State<'_, AppState>, on: bool) {
    state.engine.shared.lab_bypass.store(on, Relaxed);
}

/// Render the take through the current settings and save it as a WAV.
#[tauri::command]
async fn lab_export(state: State<'_, AppState>) -> Res<String> {
    let take = state.engine.lab_take().ok_or("Record a take first")?;
    let sr = state.engine.sample_rate();
    let shared = state.engine.shared.clone();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = state.dirs.recordings.join(format!("Voice Lab take {stamp}.wav"));
    let out = path.clone();
    blocking(move || -> Res<()> {
        let mut chain = audio::chain::VocalChain::new(sr as f32);
        // Flush the chain's latency with a little silence at the end.
        let mut input = (*take).clone();
        input.extend(std::iter::repeat(0.0).take(sr as usize / 2));
        let mut stereo = Vec::new();
        chain.process(&input, &mut stereo, &shared.params, |_| {});
        let skip = chain.latency() * 2;
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let spec = hound::WavSpec { channels: 2, sample_rate: sr, bits_per_sample: 24, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&out, spec).map_err(|e| e.to_string())?;
        for &x in stereo.iter().skip(skip) {
            w.write_sample((x.clamp(-1.0, 1.0) * 8_388_607.0) as i32).map_err(|e| e.to_string())?;
        }
        w.finalize().map_err(|e| e.to_string())
    })
    .await??;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
fn recordings_dir(state: State<'_, AppState>) -> Res<String> {
    std::fs::create_dir_all(&state.dirs.recordings).map_err(|e| e.to_string())?;
    Ok(state.dirs.recordings.to_string_lossy().into_owned())
}

// ------------------------------------------------------------------ tick --

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Tick {
    pos: f64,
    duration: f64,
    playing: bool,
    ended: bool,
    recording: bool,
    mic: f32,
    out: f32,
    /// [song seconds, sung MIDI, corrected MIDI, target note, voiced 0/1]
    pitch: Vec<(f64, f32, f32, i32, u8)>,
    score: Score,
    /// Voice Lab: [state, position s, length s]
    lab: (u32, f64, f64),
    /// Waiting for the vocal separator to catch up.
    buffering: bool,
    /// Seconds of the song ready to play.
    ready: f64,
    error: Option<String>,
}

#[derive(Serialize, Clone, Copy, Default)]
#[serde(rename_all = "camelCase")]
struct Score {
    /// 0..100, average in-tune-ness of everything sung.
    accuracy: f32,
    /// Seconds of voiced singing counted.
    sung: f32,
    points: u32,
    streak: u32,
}

fn spawn_ticker(app: AppHandle, engine: Arc<Engine>, generation: Arc<AtomicU32>) {
    std::thread::Builder::new()
        .name("ui-ticker".into())
        .spawn(move || {
            let mut score = Score::default();
            let mut acc_sum = 0.0f64;
            let mut seen_gen = generation.load(Relaxed);
            let mut last_pos = 0u64;
            let mut held = 0u32;
            loop {
                std::thread::sleep(Duration::from_millis(33));
                let sh = &engine.shared;
                let sr = sh.sample_rate.load(Relaxed).max(1) as f64;
                let playing = sh.flags.playing.load(Relaxed);
                let pos = sh.position.load(Relaxed);
                // New song or rewound to the start: fresh score.
                let gen = generation.load(Relaxed);
                if gen != seen_gen || (pos < last_pos && pos < sr as u64) {
                    seen_gen = gen;
                    score = Score::default();
                    acc_sum = 0.0;
                }
                last_pos = pos;

                let mut pitch = Vec::new();
                if let Some(cons) = engine.frames.lock().unwrap().as_mut() {
                    while let Some(f) = cons.try_pop() {
                        let fr = f.frame;
                        if fr.voiced && playing {
                            let dev = (fr.input - fr.target as f32).abs();
                            let acc = (1.0 - dev / 0.5).clamp(0.0, 1.0);
                            acc_sum += acc as f64;
                            score.sung += 0.004;
                            if acc > 0.6 {
                                held += 1;
                                score.points += 1 + (score.streak / 100).min(4);
                                if held % 25 == 0 {
                                    score.streak += 1;
                                }
                            } else {
                                held = 0;
                                score.streak = 0;
                            }
                        }
                        pitch.push((f.pos as f64 / sr, fr.input, fr.output, fr.target, fr.voiced as u8));
                    }
                }
                let voiced_frames = (score.sung / 0.004).max(1.0) as f64;
                score.accuracy = (acc_sum / voiced_frames * 100.0) as f32;

                let tick = Tick {
                    pos: pos as f64 / sr,
                    duration: sh.length.load(Relaxed) as f64 / sr,
                    playing,
                    ended: sh.ended.load(Relaxed),
                    recording: sh.flags.recording.load(Relaxed),
                    mic: sh.mic_peak.get(),
                    out: sh.out_peak.get(),
                    pitch,
                    score,
                    buffering: sh.buffering.load(Relaxed),
                    ready: engine.current_track().map(|t| t.ready_frames()).unwrap_or(0) as f64 / sr,
                    lab: (
                        sh.lab_state.load(Relaxed),
                        sh.lab_pos.load(Relaxed) as f64 / sr,
                        sh.lab_len.load(Relaxed) as f64 / sr,
                    ),
                    error: sh.stream_error.lock().unwrap().take(),
                };
                if app.emit("engine://tick", tick).is_err() {
                    break;
                }
            }
        })
        .expect("spawn ticker");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let paths = app.path();
            let data = paths.app_data_dir()?;
            let cache = paths.app_cache_dir()?.join("audio");
            let stems = paths.app_cache_dir()?.join("stems");
            let recordings = paths
                .audio_dir()
                .map(|d| d.join("Open Karaoke"))
                .unwrap_or_else(|_| data.join("recordings"));
            let dirs = Dirs { tools: data.join("bin"), cache: cache.clone(), stems: stems.clone(), recordings };
            std::fs::create_dir_all(&dirs.tools)?;
            std::fs::create_dir_all(&dirs.cache)?;
            std::fs::create_dir_all(&dirs.stems)?;

            let engine = Engine::new();
            let generation = Arc::new(AtomicU32::new(0));
            spawn_ticker(app.handle().clone(), engine.clone(), generation.clone());

            let tools_dir = dirs.tools.clone();
            tauri::async_runtime::spawn(async move {
                youtube::prune_cache(&cache, 3 * 1024 * 1024 * 1024);
                youtube::prune_cache(&stems, 3 * 1024 * 1024 * 1024);
                tools::maybe_update_ytdlp(&tools_dir).await;
            });

            app.manage(AppState {
                engine,
                dirs,
                song: Mutex::new(Song::default()),
                generation,
                separator: Arc::new(Mutex::new(None)),
                worker: Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            tools_status,
            tools_install,
            search_songs,
            get_lyrics,
            yt_search,
            yt_download,
            song_separate,
            stems_cached,
            song_vocal_removal,
            song_transform,
            song_unload,
            transport_play,
            transport_pause,
            transport_seek,
            set_params,
            audio_devices,
            audio_open,
            audio_status,
            record_start,
            record_stop,
            lab_record,
            lab_stop_record,
            lab_play,
            lab_stop,
            lab_bypass,
            lab_export,
            recordings_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Open Karaoke");
}
