export const NOTE_NAMES = ["C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B"];

/** Must stay in the same order as `SCALES` in src-tauri/src/audio/scale.rs. */
export const SCALES: { name: string; steps: number[] }[] = [
  { name: "Chromatic", steps: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11] },
  { name: "Major", steps: [0, 2, 4, 5, 7, 9, 11] },
  { name: "Minor", steps: [0, 2, 3, 5, 7, 8, 10] },
  { name: "Harmonic minor", steps: [0, 2, 3, 5, 7, 8, 11] },
  { name: "Melodic minor", steps: [0, 2, 3, 5, 7, 9, 11] },
  { name: "Dorian", steps: [0, 2, 3, 5, 7, 9, 10] },
  { name: "Phrygian", steps: [0, 1, 3, 5, 7, 8, 10] },
  { name: "Lydian", steps: [0, 2, 4, 6, 7, 9, 11] },
  { name: "Mixolydian", steps: [0, 2, 4, 5, 7, 9, 10] },
  { name: "Major pentatonic", steps: [0, 2, 4, 7, 9] },
  { name: "Minor pentatonic", steps: [0, 3, 5, 7, 10] },
  { name: "Blues", steps: [0, 3, 5, 6, 7, 10] },
];

export const SCALE_MAJOR = 1;
export const SCALE_MINOR = 2;

/** Diatonic harmony intervals understood by the engine (scale degrees, ±7 = octave). */
export const HARMONIES: { label: string; value: number }[] = [
  { label: "Off", value: 0 },
  { label: "3rd above", value: 2 },
  { label: "4th above", value: 3 },
  { label: "5th above", value: 4 },
  { label: "6th above", value: 5 },
  { label: "Octave above", value: 7 },
  { label: "3rd below", value: -2 },
  { label: "4th below", value: -3 },
  { label: "5th below", value: -4 },
  { label: "6th below", value: -5 },
  { label: "Octave below", value: -7 },
];

export const mod12 = (n: number) => ((n % 12) + 12) % 12;

export function noteName(midi: number, withOctave = true): string {
  const n = Math.round(midi);
  return NOTE_NAMES[mod12(n)] + (withOctave ? String(Math.floor(n / 12) - 1) : "");
}

export function keyName(tonic: number, scale: number): string {
  const s = SCALES[scale]?.name ?? "";
  return `${NOTE_NAMES[mod12(tonic)]} ${s.toLowerCase()}`;
}

export function inScale(midi: number, tonic: number, scale: number): boolean {
  return (SCALES[scale]?.steps ?? []).includes(mod12(midi - tonic));
}

export function formatTime(s: number): string {
  if (!isFinite(s) || s < 0) s = 0;
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${sec.toString().padStart(2, "0")}`;
}
