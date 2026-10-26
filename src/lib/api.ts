import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface ToolStatus {
  ready: boolean;
  ytdlp: boolean;
  deno: boolean;
  model: boolean;
}

export interface SongMeta {
  id: string;
  title: string;
  artist: string;
  album: string;
  duration: number;
  artwork: string | null;
  year: string | null;
  explicit: boolean;
}

export interface Lyrics {
  synced: string | null;
  plain: string | null;
  instrumental: boolean;
}

export interface Video {
  id: string;
  title: string;
  channel: string;
  duration: number | null;
  views: number | null;
}

export interface KeyEstimate {
  tonic: number;
  minor: boolean;
  confidence: number;
}

export interface SongInfo {
  duration: number;
  sourceDuration: number;
  key: KeyEstimate | null;
  semitones: number;
  tempo: number;
  hasGuide: boolean;
  /** The AI is still separating; playback can already start. */
  separating: boolean;
  vocalRemoved: boolean;
}

export interface SeparateProgress {
  /** The job this event belongs to; anything else is stale. */
  job: number;
  fraction: number;
  /** Seconds of the song ready to play. */
  ready: number;
  /** Separation speed relative to real time. */
  speed: number;
}

export interface DeviceList {
  inputs: string[];
  outputs: string[];
  defaultInput: string | null;
  defaultOutput: string | null;
}

export interface AudioStatus {
  sampleRate: number;
  output: string;
  input: string | null;
  inputError: string | null;
  buffer: number | null;
  latencyMs: number;
}

export interface Score {
  accuracy: number;
  sung: number;
  points: number;
  streak: number;
}

/** [song seconds, sung MIDI, corrected MIDI, target note, voiced 0/1] */
export type PitchPoint = [number, number, number, number, number];

export interface Tick {
  pos: number;
  duration: number;
  playing: boolean;
  ended: boolean;
  recording: boolean;
  mic: number;
  out: number;
  pitch: PitchPoint[];
  score: Score;
  /** Voice Lab: [state 0 idle / 1 recording / 2 playing, position s, length s] */
  lab: [number, number, number];
  /** Playback caught up with the separator and is waiting. */
  buffering: boolean;
  /** Seconds of the song ready to play. */
  ready: number;
  error: string | null;
}

export const api = {
  toolsStatus: () => invoke<ToolStatus>("tools_status"),
  toolsInstall: () => invoke<ToolStatus>("tools_install"),
  searchSongs: (query: string) => invoke<SongMeta[]>("search_songs", { query }),
  getLyrics: (m: SongMeta) =>
    invoke<Lyrics>("get_lyrics", { artist: m.artist, title: m.title, album: m.album, duration: m.duration }),
  ytSearch: (query: string, limit = 10) => invoke<Video[]>("yt_search", { query, limit }),
  ytDownload: (id: string) => invoke<string>("yt_download", { id }),
  songSeparate: (path: string, id: string, job: number) => invoke<SongInfo>("song_separate", { path, id, job }),
  stemsCached: (ids: string[]) => invoke<string[]>("stems_cached", { ids }),
  songVocalRemoval: (path: string) => invoke<SongInfo>("song_vocal_removal", { path }),
  songTransform: (semitones: number, tempo: number) => invoke<SongInfo>("song_transform", { semitones, tempo }),
  songUnload: () => invoke<void>("song_unload"),
  play: () => invoke<void>("transport_play"),
  pause: () => invoke<void>("transport_pause"),
  seek: (seconds: number) => invoke<void>("transport_seek", { seconds }),
  setParams: (params: [string, number][]) => invoke<string[]>("set_params", { params }),
  audioDevices: () => invoke<DeviceList>("audio_devices"),
  audioOpen: (input: string | null, output: string | null, buffer: number | null) =>
    invoke<AudioStatus>("audio_open", { input, output, buffer }),
  audioStatus: () => invoke<AudioStatus>("audio_status"),
  recordStart: (name: string) => invoke<string>("record_start", { name }),
  recordStop: () => invoke<string | null>("record_stop"),
  labRecord: () => invoke<void>("lab_record"),
  labStopRecord: () => invoke<number>("lab_stop_record"),
  labPlay: (looped: boolean) => invoke<void>("lab_play", { looped }),
  labStop: () => invoke<void>("lab_stop"),
  labBypass: (on: boolean) => invoke<void>("lab_bypass", { on }),
  labExport: () => invoke<string>("lab_export"),
  recordingsDir: () => invoke<string>("recordings_dir"),
};

export function onTick(cb: (t: Tick) => void): Promise<UnlistenFn> {
  return listen<Tick>("engine://tick", (e) => cb(e.payload));
}

export function onSetupProgress(cb: (p: { step: string; label: string; fraction: number }) => void) {
  return listen<{ step: string; label: string; fraction: number }>("setup://progress", (e) => cb(e.payload));
}

export function onSeparateProgress(cb: (p: SeparateProgress) => void) {
  return listen<SeparateProgress>("separate://progress", (e) => cb(e.payload));
}

export function onSeparateDone(cb: (e: { job: number; info: SongInfo }) => void) {
  return listen<{ job: number; info: SongInfo }>("separate://done", (e) => cb(e.payload));
}

export function onSeparateError(cb: (e: { job: number; message: string }) => void) {
  return listen<{ job: number; message: string }>("separate://error", (e) => cb(e.payload));
}

export function onDownloadProgress(cb: (p: { id: string; fraction: number }) => void) {
  return listen<{ id: string; fraction: number }>("download://progress", (e) => cb(e.payload));
}

export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e);
}
