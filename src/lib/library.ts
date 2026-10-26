import type { Lyrics } from "./api";
import type { Ranked } from "./sources";

/**
 * What it took to prepare a song, kept on this computer so reopening it
 * skips the YouTube search, the download and (with the stem cache) the AI.
 */
export interface LibraryEntry {
  sourceId: string;
  sourcePath: string;
  /** Alternative recordings, for the Song tab. */
  originals: Ranked[];
  lyrics: Lyrics | null;
  savedAt: number;
}

const KEY = "openkaraoke.library.v1";
const MAX_ENTRIES = 80;

function read(): Record<string, LibraryEntry> {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? "{}") as Record<string, LibraryEntry>;
  } catch {
    return {};
  }
}

function write(all: Record<string, LibraryEntry>) {
  // Keep the most recently used entries within budget.
  const entries = Object.entries(all).sort((a, b) => b[1].savedAt - a[1].savedAt).slice(0, MAX_ENTRIES);
  try {
    localStorage.setItem(KEY, JSON.stringify(Object.fromEntries(entries)));
  } catch {
    /* storage full or unavailable: the library is only an accelerator */
  }
}

export function libraryEntry(songId: string): LibraryEntry | null {
  return read()[songId] ?? null;
}

/** Merge `patch` into a song's entry; a source is required to create one. */
export function saveToLibrary(songId: string, patch: Partial<Omit<LibraryEntry, "savedAt">>) {
  const all = read();
  const prev: LibraryEntry | undefined = all[songId];
  if (!prev && !(patch.sourceId && patch.sourcePath)) return;
  const base: LibraryEntry = prev ?? { sourceId: "", sourcePath: "", originals: [], lyrics: null, savedAt: 0 };
  all[songId] = { ...base, ...patch, savedAt: Date.now() };
  write(all);
}

/** Source ids for songs, for asking the backend which are ready to play. */
export function librarySources(songIds: string[]): Map<string, string> {
  const all = read();
  const map = new Map<string, string>();
  for (const id of songIds) if (all[id]?.sourceId) map.set(all[id].sourceId, id);
  return map;
}
