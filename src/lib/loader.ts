import { api, errorText, onSeparateDone, onSeparateError, onSeparateProgress, type Lyrics, type SeparateProgress, type SongInfo, type SongMeta, type Video } from "./api";
import { libraryEntry, saveToLibrary } from "./library";
import { parseLrc } from "./lrc";
import { mod12, SCALE_MAJOR, SCALE_MINOR } from "./music";
import { rankOriginals, type Ranked } from "./sources";
import { resetPitchTrail, useStore, type SongState } from "./store";

let loadToken = 0;
const isCurrent = (token: number) => token === loadToken;

const st = () => useStore.getState();

/** Seconds of separated audio to have ready before singing can start. */
const HEAD_START = 30;

function applySongKey(info: SongInfo) {
  const song = st().song;
  if (!info.key || (song && !song.keyAuto)) return;
  st().setParams({
    tune_key: mod12(info.key.tonic + Math.round(info.semitones)),
    tune_scale: info.key.minor ? SCALE_MINOR : SCALE_MAJOR,
  });
}

async function settle<T>(p: Promise<T>): Promise<T | null> {
  try {
    return await p;
  } catch {
    return null;
  }
}

/**
 * How much audio must be separated before playback can start without ever
 * catching up with the separator. On fast machines that is the fixed head
 * start; on slow ones (separating slower than real time) it grows so the
 * rest of the song finishes before the singer gets there.
 */
export function startThreshold(p: SeparateProgress, duration: number): number {
  const speed = Math.max(p.speed, 0.05);
  const needed = speed >= 1.2 ? HEAD_START : duration * (1 - speed / 1.2) + HEAD_START;
  return Math.min(duration, needed);
}

/** Search for the original → download → AI separation. Lyrics load in parallel. */
export async function openSong(meta: SongMeta) {
  const token = ++loadToken;
  resetPitchTrail();
  await api.pause().catch(() => {});
  const initial: SongState = {
    meta,
    phase: "searching",
    message: "Finding the original recording…",
    progress: 0,
    info: null,
    lyrics: { status: "loading", lines: null, plain: null, instrumental: false },
    originals: [],
    sourceId: null,
    sourcePath: null,
    separation: null,
    userOffset: 0,
    keyAuto: true,
    busy: null,
  };
  useStore.setState({ song: initial, view: "stage", ended: false, pos: 0 });
  st().pushHistory(meta);

  const saved = libraryEntry(meta.id);
  const showLyrics = (l: Lyrics) => {
    const lines = l.synced ? parseLrc(l.synced) : null;
    st().patchSong({
      lyrics: {
        status: lines?.length || l.plain || l.instrumental ? "ready" : "missing",
        lines: lines?.length ? lines : null,
        plain: l.plain,
        instrumental: l.instrumental,
      },
    });
  };
  if (saved?.lyrics) {
    showLyrics(saved.lyrics);
  } else {
    api
      .getLyrics(meta)
      .then((l) => {
        if (!isCurrent(token)) return;
        showLyrics(l);
        saveToLibrary(meta.id, { lyrics: l });
      })
      .catch(() => isCurrent(token) && st().patchSong({ lyrics: { status: "missing", lines: null, plain: null, instrumental: false } }));
  }

  // Sung before: reuse the chosen recording instead of searching again.
  if (saved?.sourceId) {
    st().patchSong({ originals: saved.originals });
    const source = saved.originals.find((r) => r.video.id === saved.sourceId)?.video ?? {
      id: saved.sourceId,
      title: meta.title,
      channel: meta.artist,
      duration: meta.duration,
      views: null,
    };
    const ready = await api.stemsCached([saved.sourceId]).catch(() => [] as string[]);
    if (!isCurrent(token)) return;
    await buildFrom(source, token, ready.length ? saved.sourcePath : undefined);
    return;
  }

  const q = `${meta.artist} ${meta.title.replace(/\s*[([].*?[)\]]/g, "")}`;
  const [official, plain] = await Promise.all([settle(api.ytSearch(`${q} official audio`, 8)), settle(api.ytSearch(`${q} audio`, 6))]);
  if (!isCurrent(token)) return;
  if (!official && !plain) {
    st().patchSong({ phase: "error", message: "Couldn't reach YouTube. Check your connection and try again." });
    return;
  }
  const originals = rankOriginals([...(official ?? []), ...(plain ?? [])], meta);
  st().patchSong({ originals });
  saveToLibrary(meta.id, { originals: originals.slice(0, 6) });
  if (!originals.length) {
    st().patchSong({ phase: "error", message: "No recording of this song was found." });
    return;
  }
  await buildFrom(originals[0].video, token);
}

/**
 * Prepare `video` as the song's recording. `cachedPath` skips the download
 * when the separated stems are already on disk (the backend then never
 * needs the original file).
 */
async function buildFrom(video: Video, token: number, cachedPath?: string) {
  let path = cachedPath ?? "";
  if (!cachedPath) {
    st().patchSong({ phase: "downloading", message: "Downloading the original recording…", progress: 0 });
    try {
      path = await api.ytDownload(video.id);
    } catch (e) {
      if (isCurrent(token)) st().patchSong({ phase: "error", message: errorText(e) });
      return;
    }
    if (!isCurrent(token)) return;
  }
  st().patchSong({
    phase: "separating",
    message: "Separating the vocals with AI…",
    sourceId: video.id,
    sourcePath: path,
    separation: { job: token, fraction: 0, ready: 0, speed: 0 },
  });
  try {
    // The load token doubles as the job id, so events from a song the user
    // has already left can be recognised and ignored.
    const info = await api.songSeparate(path, video.id, token);
    if (!isCurrent(token)) return;
    const songId = st().song?.meta.id;
    if (songId) {
      const originals = st().song?.originals ?? [];
      saveToLibrary(songId, { sourceId: video.id, sourcePath: path, originals: originals.slice(0, 6) });
    }
    applySongKey(info);
    // Cached songs come back complete; otherwise wait for the head start.
    st().patchSong(info.separating ? { info } : { info, phase: "ready", separation: null });
  } catch (e) {
    if (!isCurrent(token) || errorText(e) === "superseded") return;
    await fallBackToRemover(path, errorText(e), token);
  }
}

/** When the AI can't run, the basic remover still gives a usable track. */
async function fallBackToRemover(path: string, reason: string, token: number) {
  st().toast(`AI separation unavailable (${reason}). Using the basic vocal remover instead.`, "error");
  st().patchSong({ phase: "separating", message: "Removing the vocals…", separation: null });
  try {
    const info = await api.songVocalRemoval(path);
    if (!isCurrent(token)) return;
    applySongKey(info);
    st().patchSong({ info, phase: "ready" });
  } catch (e) {
    if (isCurrent(token) && errorText(e) !== "superseded") st().patchSong({ phase: "error", message: errorText(e) });
  }
}

/** Route separator events to the current song. Call once at startup. */
export function listenForSeparation() {
  const progress = onSeparateProgress((p) => {
    const song = st().song;
    if (p.job !== loadToken || !song?.info?.separating) return;
    const patch: Partial<SongState> = { separation: p };
    if (song.phase === "separating" && p.ready >= startThreshold(p, song.info.sourceDuration)) patch.phase = "ready";
    st().patchSong(patch);
  });
  const done = onSeparateDone(({ job, info }) => {
    const song = st().song;
    if (job !== loadToken || !song) return;
    st().patchSong({ info, separation: null, phase: song.phase === "separating" ? "ready" : song.phase });
  });
  const failed = onSeparateError(({ job, message }) => {
    const song = st().song;
    if (job !== loadToken || !song?.sourcePath) return;
    void api.pause();
    void fallBackToRemover(song.sourcePath, message, job);
  });
  return () => {
    for (const un of [progress, done, failed]) void un.then((f) => f());
  };
}

/** Use a different recording of the song (e.g. if the first had an intro). */
export async function switchSource(r: Ranked) {
  const token = ++loadToken;
  await api.pause().catch(() => {});
  resetPitchTrail();
  st().patchSong({ userOffset: 0, info: null });
  await buildFrom(r.video, token);
}

export async function switchToVocalRemover() {
  const path = st().song?.sourcePath;
  if (!path) return;
  const token = ++loadToken;
  await api.pause().catch(() => {});
  st().patchSong({ phase: "separating", message: "Removing the vocals…", separation: null });
  try {
    const info = await api.songVocalRemoval(path);
    if (!isCurrent(token)) return;
    applySongKey(info);
    st().patchSong({ info, phase: "ready" });
  } catch (e) {
    if (isCurrent(token) && errorText(e) !== "superseded") st().patchSong({ phase: "error", message: errorText(e) });
  }
}

export async function transform(semitones: number, tempo: number) {
  const song = st().song;
  if (!song?.info) return;
  st().patchSong({ busy: semitones !== song.info.semitones ? "Changing key…" : "Changing tempo…" });
  try {
    const info = await api.songTransform(semitones, tempo);
    st().patchSong({ info, busy: null });
    applySongKey(info);
  } catch (e) {
    st().patchSong({ busy: null });
    if (errorText(e) !== "superseded") st().toast(errorText(e), "error");
  }
}

export function closeSong() {
  loadToken++;
  void api.songUnload();
  useStore.setState({ song: null, view: "search", playing: false });
}
