export type Params = Record<string, number>;

/** Mirrors the defaults in src-tauri/src/audio/params.rs. */
export const DEFAULT_PARAMS: Params = {
  tune_enabled: 1,
  tune_key: 0,
  tune_scale: 1,
  tune_retune_ms: 40,
  tune_amount: 1,
  tune_humanize: 0.3,
  tune_flex: 0,
  tune_vibrato_depth: 0,
  tune_vibrato_rate: 5.5,
  tune_transpose: 0,
  tune_formant: 0,
  tune_mix: 1,
  tune_sensitivity: 0.8,
  harmony_a: 0,
  harmony_b: 0,
  harmony_level: 0.5,
  harmony_spread: 0.7,
  doubler: 0,
  gate_db: -50,
  hpf_hz: 90,
  eq_low_db: 0,
  eq_presence_db: 2,
  eq_air_db: 2,
  deesser: 0.4,
  comp: 0.5,
  delay_mix: 0,
  delay_ms: 320,
  delay_feedback: 0.3,
  reverb_mix: 0.18,
  reverb_size: 0.7,
  reverb_damp: 0.4,
  vocal_gain_db: 0,
  master: 0.9,
  instrumental: 0.85,
  guide: 0,
  vocal: 0.9,
  monitor: 1,
  record_latency_ms: -1,
  drone: 0,
};

/** Parameters that make up a user's "sound" (saved in My presets). */
export const SOUND_KEYS = Object.keys(DEFAULT_PARAMS).filter(
  (k) => (k.startsWith("tune_") || k.startsWith("harmony_") || k === "doubler" || k.startsWith("gate_") || k.startsWith("hpf_") ||
    k.startsWith("eq_") || k === "deesser" || k === "comp" || k.startsWith("delay_") || k.startsWith("reverb_") || k === "vocal_gain_db") &&
    k !== "tune_key" && k !== "tune_scale",
);

export interface Preset {
  name: string;
  hint: string;
  values: Params;
}

export const TUNE_PRESETS: Preset[] = [
  {
    name: "Natural",
    hint: "Gentle, transparent correction",
    values: { tune_enabled: 1, tune_retune_ms: 90, tune_humanize: 0.6, tune_flex: 25, tune_amount: 0.85, tune_vibrato_depth: 0, harmony_a: 0, harmony_b: 0, doubler: 0, tune_formant: 0 },
  },
  {
    name: "Pop polish",
    hint: "Radio-ready, still human",
    values: { tune_enabled: 1, tune_retune_ms: 35, tune_humanize: 0.35, tune_flex: 8, tune_amount: 1, tune_vibrato_depth: 0, harmony_a: 0, harmony_b: 0, doubler: 0.25, tune_formant: 0 },
  },
  {
    name: "Hard tune",
    hint: "The classic robotic snap",
    values: { tune_enabled: 1, tune_retune_ms: 0, tune_humanize: 0, tune_flex: 0, tune_amount: 1, tune_vibrato_depth: 0, harmony_a: 0, harmony_b: 0, doubler: 0, tune_formant: 0 },
  },
  {
    name: "Choir",
    hint: "Stacked diatonic harmonies",
    values: { tune_enabled: 1, tune_retune_ms: 25, tune_humanize: 0.3, tune_flex: 0, tune_amount: 1, harmony_a: 2, harmony_b: -3, harmony_level: 0.55, harmony_spread: 0.9, doubler: 0.35, tune_formant: 0 },
  },
  {
    name: "Diva",
    hint: "Smooth with lush vibrato",
    values: { tune_enabled: 1, tune_retune_ms: 30, tune_humanize: 0.5, tune_flex: 0, tune_amount: 1, tune_vibrato_depth: 35, tune_vibrato_rate: 5.6, harmony_a: 0, harmony_b: 0, doubler: 0.15, tune_formant: 0 },
  },
  {
    name: "Chipmunk",
    hint: "Up an octave, tiny formants",
    values: { tune_enabled: 1, tune_retune_ms: 10, tune_transpose: 12, tune_formant: 7, harmony_a: 0, harmony_b: 0, doubler: 0 },
  },
  {
    name: "Monster",
    hint: "Deep and gravelly",
    values: { tune_enabled: 1, tune_retune_ms: 10, tune_transpose: -10, tune_formant: -6, harmony_a: 0, harmony_b: 0, doubler: 0.2 },
  },
];

export const FX_PRESETS: Preset[] = [
  {
    name: "Dry",
    hint: "Just your voice",
    values: { reverb_mix: 0, delay_mix: 0, comp: 0.3, eq_presence_db: 0, eq_air_db: 0, deesser: 0.2 },
  },
  {
    name: "Studio",
    hint: "Polished vocal booth",
    values: { reverb_mix: 0.14, reverb_size: 0.55, reverb_damp: 0.5, delay_mix: 0, comp: 0.55, eq_low_db: -1, eq_presence_db: 2.5, eq_air_db: 3, deesser: 0.45 },
  },
  {
    name: "Arena",
    hint: "Huge hall with slap echo",
    values: { reverb_mix: 0.32, reverb_size: 0.92, reverb_damp: 0.3, delay_mix: 0.18, delay_ms: 380, delay_feedback: 0.35, comp: 0.6, eq_presence_db: 3, eq_air_db: 3.5, deesser: 0.5 },
  },
  {
    name: "Club echo",
    hint: "Rhythmic ping-pong delay",
    values: { reverb_mix: 0.16, reverb_size: 0.6, delay_mix: 0.32, delay_ms: 250, delay_feedback: 0.45, comp: 0.6, eq_presence_db: 2, eq_air_db: 2 },
  },
  {
    name: "Radio",
    hint: "Thin and punchy",
    values: { reverb_mix: 0.05, delay_mix: 0, comp: 0.9, hpf_hz: 300, eq_low_db: -6, eq_presence_db: 6, eq_air_db: -4, deesser: 0.3 },
  },
];
