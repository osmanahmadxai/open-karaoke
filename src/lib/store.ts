import { create } from "zustand";
import { api, type AudioStatus, type PitchPoint, type Score, type SongInfo, type SongMeta, type Tick, type ToolStatus } from "./api";
import type { Line } from "./lrc";
import { DEFAULT_PARAMS, type Params } from "./presets";
import type { Ranked } from "./sources";

export type View = "boot" | "setup" | "search" | "stage" | "lab";
export type Phase = "searching" | "downloading" | "analyzing" | "ready" | "error";
export type Tab = "tune" | "voice" | "mix" | "song";

export interface LyricsState {
  status: "loading" | "ready" | "missing";
  lines: Line[] | null;
  plain: string | null;
  instrumental: boolean;
}

export interface SongState {
  meta: SongMeta;
  phase: Phase;
  message: string;
  progress: number;
  info: SongInfo | null;
  lyrics: LyricsState;
  instrumentals: Ranked[];
  originals: Ranked[];
  instId: string | null;
  origId: string | null;
  origPath: string | null;
  sync: "pending" | "synced" | "unsynced" | "unavailable";
  /** Manual lyric nudge on top of the automatic alignment, seconds. */
  userOffset: number;
  /** Autotune key follows the detected song key. */
  keyAuto: boolean;
  busy: string | null;
}

export interface Toast {
  id: number;
  text: string;
  kind: "info" | "error" | "success";
  action?: { label: string; run: () => void };
}

export interface AudioPrefs {
  input: string | null;
  output: string | null;
  buffer: number | null;
}

const PERSIST_KEY = "croonly.params.v1";
const PREFS_KEY = "croonly.audio.v1";
const HISTORY_KEY = "croonly.history.v1";
// Per-song values that should not survive a restart.
const TRANSIENT = new Set(["tune_key", "tune_scale", "drone"]);
const MY_PRESETS_KEY = "croonly.mypresets.v1";

export interface MyPreset {
  name: string;
  values: Params;
}

function load<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? { ...fallback, ...JSON.parse(raw) } : fallback;
  } catch {
    return fallback;
  }
}

function loadList<T>(key: string): T[] {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T[]) : [];
  } catch {
    return [];
  }
}

interface Store {
  view: View;
  tools: ToolStatus | null;
  audio: AudioStatus | null;
  audioPrefs: AudioPrefs;
  params: Params;
  song: SongState | null;
  history: SongMeta[];
  pos: number;
  duration: number;
  playing: boolean;
  ended: boolean;
  recording: boolean;
  mic: number;
  out: number;
  score: Score;
  toasts: Toast[];
  tab: Tab;
  sidebar: boolean;
  settingsOpen: boolean;
  lastRecording: string | null;
  /** Voice Lab transport, mirrored from ticks. */
  lab: { state: number; pos: number; len: number };
  myPresets: MyPreset[];

  savePreset: (name: string, values: Params) => void;
  deletePreset: (name: string) => void;
  setView: (v: View) => void;
  setParam: (name: string, value: number) => void;
  setParams: (p: Params) => void;
  patchSong: (p: Partial<SongState>) => void;
  toast: (text: string, kind?: Toast["kind"], action?: Toast["action"]) => void;
  dismiss: (id: number) => void;
  pushHistory: (m: SongMeta) => void;
  set: (p: Partial<Store>) => void;
}

let pending: Params = {};
let flushQueued = false;

function queueParams(p: Params) {
  Object.assign(pending, p);
  if (flushQueued) return;
  flushQueued = true;
  requestAnimationFrame(() => {
    const batch = Object.entries(pending) as [string, number][];
    pending = {};
    flushQueued = false;
    api.setParams(batch).catch(() => {});
  });
}

function persist(params: Params) {
  const keep: Params = {};
  for (const [k, v] of Object.entries(params)) if (!TRANSIENT.has(k)) keep[k] = v;
  try {
    localStorage.setItem(PERSIST_KEY, JSON.stringify(keep));
  } catch {
    /* storage unavailable */
  }
}

let toastId = 0;

export const useStore = create<Store>((set, get) => ({
  view: "boot",
  tools: null,
  audio: null,
  audioPrefs: load<AudioPrefs>(PREFS_KEY, { input: null, output: null, buffer: 256 }),
  params: load<Params>(PERSIST_KEY, DEFAULT_PARAMS),
  song: null,
  history: loadList<SongMeta>(HISTORY_KEY),
  pos: 0,
  duration: 0,
  playing: false,
  ended: false,
  recording: false,
  mic: 0,
  out: 0,
  score: { accuracy: 0, sung: 0, points: 0, streak: 0 },
  toasts: [],
  tab: "tune",
  sidebar: true,
  settingsOpen: false,
  lastRecording: null,
  lab: { state: 0, pos: 0, len: 0 },
  myPresets: loadList<MyPreset>(MY_PRESETS_KEY),

  savePreset: (name, values) => {
    const myPresets = [...get().myPresets.filter((p) => p.name !== name), { name, values }];
    set({ myPresets });
    try {
      localStorage.setItem(MY_PRESETS_KEY, JSON.stringify(myPresets));
    } catch {
      /* storage unavailable */
    }
  },
  deletePreset: (name) => {
    const myPresets = get().myPresets.filter((p) => p.name !== name);
    set({ myPresets });
    try {
      localStorage.setItem(MY_PRESETS_KEY, JSON.stringify(myPresets));
    } catch {
      /* storage unavailable */
    }
  },
  setView: (view) => set({ view }),
  setParam: (name, value) => get().setParams({ [name]: value }),
  setParams: (p) => {
    const params = { ...get().params, ...p };
    set({ params });
    queueParams(p);
    persist(params);
  },
  patchSong: (p) => {
    const s = get().song;
    if (s) set({ song: { ...s, ...p } });
  },
  toast: (text, kind = "info", action) => {
    const id = ++toastId;
    set({ toasts: [...get().toasts, { id, text, kind, action }].slice(-4) });
    setTimeout(() => get().dismiss(id), kind === "error" ? 8000 : action ? 9000 : 4500);
  },
  dismiss: (id) => set({ toasts: get().toasts.filter((t) => t.id !== id) }),
  pushHistory: (m) => {
    const history = [m, ...get().history.filter((h) => h.id !== m.id)].slice(0, 12);
    set({ history });
    try {
      localStorage.setItem(HISTORY_KEY, JSON.stringify(history));
    } catch {
      /* storage unavailable */
    }
  },
  set: (p) => {
    set(p);
    if (p.audioPrefs) {
      try {
        localStorage.setItem(PREFS_KEY, JSON.stringify(p.audioPrefs));
      } catch {
        /* storage unavailable */
      }
    }
  },
}));

/** Push every parameter to the engine (startup, after reconnecting). */
export function syncAllParams() {
  queueParams(useStore.getState().params);
}

// ---------------------------------------------------------------- clock --
// Ticks arrive ~30×/s; animation interpolates between them.

const clock = { pos: 0, at: 0, playing: false };

export function songTime(): number {
  if (!clock.playing) return clock.pos;
  // The engine plays the rendered (tempo-adjusted) track in real time.
  return clock.pos + Math.min(0.12, (performance.now() - clock.at) / 1000);
}

// ---------------------------------------------------------------- pitch --

/** Pitch history; element 0 is wall-clock seconds (performance.now). */
export const pitchTrail: PitchPoint[] = [];
const PITCH_HOP = 0.004;

export function resetPitchTrail() {
  pitchTrail.length = 0;
}

export function handleTick(t: Tick) {
  clock.pos = t.pos;
  clock.at = performance.now();
  clock.playing = t.playing;
  if (t.pitch.length) {
    // Re-stamp on a wall clock (one analysis hop ≈ 4 ms apart) so the graph
    // keeps scrolling while the song is paused.
    const wall = performance.now() / 1000;
    const n = t.pitch.length;
    t.pitch.forEach((p, k) => pitchTrail.push([wall - (n - 1 - k) * PITCH_HOP, p[1], p[2], p[3], p[4]]));
    if (pitchTrail.length > 3000) pitchTrail.splice(0, pitchTrail.length - 3000);
  }
  const s = useStore.getState();
  const patch: Partial<Store> = {};
  if (Math.abs(s.pos - t.pos) > 0.05) patch.pos = t.pos;
  if (s.duration !== t.duration) patch.duration = t.duration;
  if (s.playing !== t.playing) patch.playing = t.playing;
  if (s.ended !== t.ended) patch.ended = t.ended;
  if (s.recording !== t.recording) patch.recording = t.recording;
  if (Math.abs(s.mic - t.mic) > 0.01) patch.mic = t.mic;
  if (Math.abs(s.out - t.out) > 0.01) patch.out = t.out;
  if (s.score.points !== t.score.points || s.score.streak !== t.score.streak) patch.score = t.score;
  const [ls, lp, ll] = t.lab;
  if (s.lab.state !== ls || Math.abs(s.lab.pos - lp) > 0.05 || s.lab.len !== ll) patch.lab = { state: ls, pos: lp, len: ll };
  if (Object.keys(patch).length) useStore.setState(patch);
  if (t.error) s.toast(t.error, "error");
}
