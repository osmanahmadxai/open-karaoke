import type { SongMeta, Video } from "./api";

const norm = (s: string) =>
  s
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/[^a-z0-9 ]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();

/** Core title words, ignoring "(feat. …)" / "- Remastered" style decorations. */
function titleWords(title: string): string[] {
  const core = title.replace(/\s[-–—]\s.*$/, "").replace(/[([].*?[)\]]/g, "");
  return norm(core)
    .split(" ")
    .filter((w) => w.length > 1);
}

function hasAny(text: string, words: string[]) {
  return words.some((w) => text.includes(w));
}

// Versions that are not the song as recorded.
const ALTERED = ["slowed", "sped up", "speed up", "nightcore", "8d", "reverb", "remix", "bass boosted", "lofi", "lo fi", "mashup", "1 hour", "loop", "tiktok", "chipmunk"];
const PERFORMED = ["cover", "live", "piano", "guitar", "acoustic", "orchestra", "violin", "cello", "ukulele", "drum", "tutorial", "lesson", "how to", "reaction", "music box", "marimba", "8 bit", "8bit"];

export interface Ranked {
  video: Video;
  score: number;
}

function common(v: Video, song: SongMeta) {
  const t = norm(v.title);
  const songTitle = norm(song.title);
  const words = titleWords(song.title);
  const matched = words.filter((w) => t.includes(w)).length / Math.max(1, words.length);
  let score = matched * 40;
  if (hasAny(t, norm(song.artist).split(" ").filter((w) => w.length > 2))) score += 10;
  for (const w of ALTERED) if (t.includes(w) && !songTitle.includes(w)) score -= 60;
  for (const w of PERFORMED) if (t.includes(w) && !songTitle.includes(w)) score -= 35;
  if (v.duration && song.duration > 0) {
    const d = Math.abs(v.duration - song.duration);
    score -= d < 4 ? 0 : d < 15 ? (d - 4) * 1.5 : d < 40 ? 16 + (d - 15) * 2 : 80;
  }
  if (v.duration && v.duration > 15 * 60) score -= 100;
  return { t, score };
}

export function rankOriginals(videos: Video[], song: SongMeta): Ranked[] {
  return dedupe(videos)
    .map((video) => {
      let { t, score } = common(video, song);
      if (t.includes("official audio")) score += 25;
      else if (t.includes("official")) score += 10;
      if (t.includes("audio")) score += 5;
      // The AI needs the real vocal mix, and lyrics are timed to the original.
      if (hasAny(t, ["instrumental", "karaoke", "backing track", "no vocals", "acapella", "a cappella"])) score -= 80;
      if (t.includes("lyrics") || t.includes("lyric video")) score += 3; // usually the exact album audio
      if (t.includes("music video")) score -= 5; // videos often have intros/outros
      return { video, score };
    })
    .sort((a, b) => b.score - a.score);
}

function dedupe(videos: Video[]): Video[] {
  const seen = new Set<string>();
  return videos.filter((v) => (seen.has(v.id) ? false : (seen.add(v.id), true)));
}
