pub mod audio;
mod meta;
mod tools;
mod youtube;

use audio::analysis::{self, Alignment, KeyEstimate};
use audio::engine::{self, AudioStatus, DeviceChoice, DeviceList, Engine, Track};
use ringbuf::traits::Consumer;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

struct Dirs {
    tools: PathBuf,
    cache: PathBuf,
    recordings: PathBuf,
}

#[derive(Default)]
struct Song {
    inst_path: Option<PathBuf>,
    guide_path: Option<PathBuf>,
    /// Decoded, untransformed audio at `sr`.
    inst: Option<Arc<Vec<f32>>>,
    guide: Option<Arc<Vec<f32>>>,
    /// Seconds: instrumental time = original time + offset.
    offset: f64,
    alignment: Option<Alignment>,
    key: Option<KeyEstimate>,
    semitones: f32,
    tempo: f32,
    sr: u32,
    vocal_removed: bool,
}

struct AppState {
    engine: Arc<Engine>,
    dirs: Dirs,
    song: Mutex<Song>,
    /// Bumped whenever the loaded audio changes; stale background work is dropped.
    generation: Arc<AtomicU32>,
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
    offset: f64,
    alignment: Option<Alignment>,
    vocal_removed: bool,
}

fn info(song: &Song) -> SongInfo {
    let frames = song.inst.as_ref().map(|a| a.len() / 2).unwrap_or(0) as f64;
    let sr = song.sr.max(1) as f64;
    SongInfo {
        duration: frames / sr / song.tempo as f64,
        source_duration: frames / sr,
        key: song.key,
        semitones: song.semitones,
        tempo: song.tempo,
        has_guide: song.guide.is_some(),
        offset: song.offset,
        alignment: song.alignment,
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
    let (inst, guide, semis, tempo, offset, sr) = {
        let s = state.song.lock().unwrap();
        let Some(inst) = s.inst.clone() else { return Err("No song loaded".into()) };
        (inst, s.guide.clone(), s.semitones, s.tempo, s.offset, s.sr)
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
        audio: Arc::new(audio),
        guide: guide.map(Arc::new),
        guide_offset: (offset * sr as f64 / tempo as f64).round() as i64,
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

#[tauri::command]
async fn song_load(state: State<'_, AppState>, path: String) -> Res<SongInfo> {
    state.engine.pause();
    let gen = state.generation.fetch_add(1, Relaxed) + 1;
    let sr = state.engine.sample_rate();
    let p = PathBuf::from(&path);
    let (inst, key) = blocking(move || -> Res<_> {
        let audio = audio::decode::decode_file(&p, sr)?;
        let key = analysis::detect_key(&analysis::mono(&audio), sr);
        Ok((audio, key))
    })
    .await??;
    if state.generation.load(Relaxed) != gen {
        return Err("superseded".into());
    }
    {
        let mut s = state.song.lock().unwrap();
        *s = Song {
            inst_path: Some(path.into()),
            inst: Some(Arc::new(inst)),
            key: Some(key),
            semitones: 0.0,
            tempo: 1.0,
            sr,
            ..Default::default()
        };
    }
    state.engine.set_track(None, 0);
    apply_track(&state).await
}

/// Load the original recording: used for auto-syncing lyrics, as an optional
/// guide vocal, and as the source for basic vocal removal.
#[tauri::command]
async fn song_load_guide(state: State<'_, AppState>, path: String) -> Res<SongInfo> {
    let gen = state.generation.load(Relaxed);
    let (sr, inst) = {
        let s = state.song.lock().unwrap();
        (s.sr, s.inst.clone())
    };
    let p = PathBuf::from(&path);
    let (guide, alignment) = blocking(move || -> Res<_> {
        let guide = audio::decode::decode_file(&p, sr)?;
        let alignment = inst.and_then(|i| analysis::align(&analysis::mono(&i), &analysis::mono(&guide), sr, 45.0));
        Ok((guide, alignment))
    })
    .await??;
    if state.generation.load(Relaxed) != gen {
        return Err("superseded".into());
    }
    {
        let mut s = state.song.lock().unwrap();
        s.guide_path = Some(path.into());
        s.guide = Some(Arc::new(guide));
        s.alignment = alignment;
        s.offset = alignment.filter(|a| a.confidence >= 0.2).map(|a| a.offset).unwrap_or(0.0);
    }
    apply_track(&state).await
}

/// Fallback when no instrumental exists: derive one from the original.
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
    {
        let mut s = state.song.lock().unwrap();
        *s = Song {
            inst_path: None,
            guide_path: Some(path.into()),
            inst: Some(Arc::new(inst)),
            guide: Some(Arc::new(guide)),
            key: Some(key),
            semitones: 0.0,
            tempo: 1.0,
            sr,
            vocal_removed: true,
            ..Default::default()
        };
    }
    state.engine.set_track(None, 0);
    apply_track(&state).await
}

#[tauri::command]
async fn song_transform(state: State<'_, AppState>, semitones: f32, tempo: f32) -> Res<SongInfo> {
    {
        let mut s = state.song.lock().unwrap();
        s.semitones = semitones.clamp(-12.0, 12.0);
        s.tempo = tempo.clamp(0.5, 1.5);
    }
    apply_track(&state).await
}

#[tauri::command]
fn song_set_offset(state: State<'_, AppState>, offset: f64) -> Res<()> {
    // Only the guide alignment depends on it; avoid a full re-render.
    let mut s = state.song.lock().unwrap();
    s.offset = offset;
    if let Some(t) = state.engine.current_track() {
        let track = Track {
            audio: t.audio.clone(),
            guide: t.guide.clone(),
            guide_offset: (offset * s.sr as f64 / s.tempo as f64).round() as i64,
        };
        let pos = state.engine.shared.position.load(Relaxed);
        state.engine.set_track(Some(Arc::new(track)), pos);
    }
    Ok(())
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
async fn audio_open(state: State<'_, AppState>, input: Option<String>, output: Option<String>, buffer: Option<u32>) -> Res<AudioStatus> {
    let engine = state.engine.clone();
    let status = blocking(move || engine.open(DeviceChoice { input, output, buffer })).await??;
    // A new device may run at a different rate; re-decode the song if so.
    let reload = {
        let s = state.song.lock().unwrap();
        (s.inst.is_some() && s.sr != status.sample_rate).then(|| (s.inst_path.clone(), s.guide_path.clone(), s.vocal_removed))
    };
    if let Some((inst, guide, removed)) = reload {
        let path = |p: Option<PathBuf>| p.map(|p| p.to_string_lossy().into_owned());
        if removed {
            if let Some(g) = path(guide) {
                song_vocal_removal(state.clone(), g).await?;
            }
        } else if let Some(i) = path(inst) {
            song_load(state.clone(), i).await?;
            if let Some(g) = path(guide) {
                song_load_guide(state.clone(), g).await?;
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
            let recordings = paths
                .audio_dir()
                .map(|d| d.join("Croonly"))
                .unwrap_or_else(|_| data.join("recordings"));
            let dirs = Dirs { tools: data.join("bin"), cache: cache.clone(), recordings };
            std::fs::create_dir_all(&dirs.tools)?;
            std::fs::create_dir_all(&dirs.cache)?;

            let engine = Engine::new();
            let generation = Arc::new(AtomicU32::new(0));
            spawn_ticker(app.handle().clone(), engine.clone(), generation.clone());

            let tools_dir = dirs.tools.clone();
            tauri::async_runtime::spawn(async move {
                youtube::prune_cache(&cache, 3 * 1024 * 1024 * 1024);
                tools::maybe_update_ytdlp(&tools_dir).await;
            });

            app.manage(AppState { engine, dirs, song: Mutex::new(Song::default()), generation });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            tools_status,
            tools_install,
            search_songs,
            get_lyrics,
            yt_search,
            yt_download,
            song_load,
            song_load_guide,
            song_vocal_removal,
            song_transform,
            song_set_offset,
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
        .expect("error while running Croonly");
}
