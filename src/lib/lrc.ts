export interface Word {
  text: string;
  start: number;
  end: number;
}

export interface Line {
  start: number;
  /** When the line should be fully "sung" (end of the wipe). */
  end: number;
  /** When the next line starts. */
  next: number;
  text: string;
  words: Word[];
}

const LINE_TAG = /\[(\d+):(\d+(?:[.:]\d+)?)\]/g;
const WORD_TAG = /<(\d+):(\d+(?:[.:]\d+)?)>/g;

function toSeconds(m: string, s: string): number {
  return parseInt(m, 10) * 60 + parseFloat(s.replace(":", "."));
}

/**
 * Parse LRC (including the "enhanced" per-word `<mm:ss.xx>` variant).
 * Lines without word timing get an estimated wipe: words share the line's
 * singing time in proportion to their length, and the line is assumed to be
 * sung at a natural pace rather than stretched over an instrumental break.
 */
export function parseLrc(src: string): Line[] {
  let offset = 0;
  const raw: { start: number; text: string; wordTimes: { t: number; text: string }[] }[] = [];
  for (const rawLine of src.split(/\r?\n/)) {
    const off = rawLine.match(/^\[offset:\s*([+-]?\d+)\]/i);
    if (off) {
      offset = parseInt(off[1], 10) / 1000;
      continue;
    }
    const stamps: number[] = [];
    let m: RegExpExecArray | null;
    LINE_TAG.lastIndex = 0;
    let lastIndex = 0;
    while ((m = LINE_TAG.exec(rawLine)) && m.index === lastIndex) {
      stamps.push(toSeconds(m[1], m[2]));
      lastIndex = LINE_TAG.lastIndex;
    }
    if (!stamps.length) continue;
    const body = rawLine.slice(lastIndex);
    const wordTimes: { t: number; text: string }[] = [];
    WORD_TAG.lastIndex = 0;
    const parts = body.split(WORD_TAG);
    // split() with two capture groups yields [text, m, s, text, m, s, text...]
    if (parts.length > 1) {
      for (let i = 1; i + 2 < parts.length + 1; i += 3) {
        const t = toSeconds(parts[i], parts[i + 1]);
        const text = parts[i + 2] ?? "";
        if (text.trim()) wordTimes.push({ t, text });
      }
    }
    const text = body.replace(WORD_TAG, "").replace(/\s+/g, " ").trim();
    for (const s of stamps) raw.push({ start: s - offset, text, wordTimes });
  }
  raw.sort((a, b) => a.start - b.start);

  const lines: Line[] = [];
  for (let i = 0; i < raw.length; i++) {
    const r = raw[i];
    if (!r.text) continue;
    const next = raw.slice(i + 1).find((x) => x.text)?.start ?? r.start + 8;
    let words: Word[];
    let end: number;
    if (r.wordTimes.length) {
      words = r.wordTimes.map((w, k) => ({
        text: w.text.trim(),
        start: w.t - offset,
        end: k + 1 < r.wordTimes.length ? r.wordTimes[k + 1].t - offset : Math.min(next, w.t - offset + 1.2),
      }));
      end = words[words.length - 1].end;
    } else {
      const tokens = r.text.split(" ");
      const chars = tokens.reduce((a, t) => a + t.length + 1, 0);
      const natural = Math.max(1.2, chars * 0.075);
      const span = Math.max(0.5, Math.min(next - r.start - 0.15, natural));
      end = r.start + span;
      let t = r.start;
      words = tokens.map((text) => {
        const d = (span * (text.length + 1)) / chars;
        const w = { text, start: t, end: t + d };
        t += d;
        return w;
      });
    }
    lines.push({ start: r.start, end, next, text: r.text, words });
  }
  return lines;
}

/** Index of the line active at time `t` (the last one that has started). */
export function lineAt(lines: Line[], t: number): number {
  let lo = 0;
  let hi = lines.length - 1;
  let ans = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (lines[mid].start <= t) {
      ans = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  return ans;
}
