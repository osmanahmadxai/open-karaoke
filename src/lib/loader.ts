import { api, errorText, type SongInfo, type SongMeta, type Video } from "./api";
import { parseLrc } from "./lrc";
import { mod12, SCALE_MAJOR, SCALE_MINOR } from "./music";
import { INSTRUMENTAL_THRESHOLD, rankInstrumentals, rankOriginals, type Ranked } from "./sources";
import { resetPitchTrail, useStore, type SongState } from "./store";

let loadToken = 0;
const isCurrent = (token: number) => token === loadToken;

const st = () => useStore.getState();

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

/** Search → pick sources → download → decode/analyse. Lyrics load in parallel. */
export async function openSong(meta: SongMeta) {
  const token = ++loadToken;
  resetPitchTrail();
  await api.pause().catch(() => {});
  const initial: SongState = {
    meta,
    phase: "searching",
    message: "Finding the best instrumental…",
    progress: 0,
    info: null,
    lyrics: { status: "loading", lines: null, plain: null, instrumental: false },
    instrumentals: [],
    originals: [],
    instId: null,
    origId: null,
    origPath: null,
    sync: "pending",
    userOffset: 0,
    keyAuto: true,
    busy: null,
  };
  useStore.setState({ song: initial, view: "stage", ended: false, pos: 0 });
  st().pushHistory(meta);

  api
    .getLyrics(meta)
    .then((l) => {
      if (!isCurrent(token)) return;
      const lines = l.synced ? parseLrc(l.synced) : null;
      st().patchSong({
        lyrics: {
          status: lines?.length || l.plain || l.instrumental ? "ready" : "missing",
          lines: lines?.length ? lines : null,
          plain: l.plain,
          instrumental: l.instrumental,
        },
      });
    })
    .catch(() => isCurrent(token) && st().patchSong({ lyrics: { status: "missing", lines: null, plain: null, instrumental: false } }));

  const q = `${meta.artist} ${meta.title.replace(/\s*[([].*?[)\]]/g, "")}`;
  const [inst, kara, orig] = await Promise.all([
    settle(api.ytSearch(`${q} instrumental`, 10)),
    settle(api.ytSearch(`${q} karaoke`, 6)),
    settle(api.ytSearch(`${q} official audio`, 6)),
  ]);
  if (!isCurrent(token)) return;
  if (!inst && !kara && !orig) {
    st().patchSong({ phase: "error", message: "Couldn't reach YouTube. Check your connection and try again." });
    return;
  }
  const instrumentals = rankInstrumentals([...(inst ?? []), ...(kara ?? [])], meta);
  const originals = rankOriginals(orig ?? [], meta);
  st().patchSong({ instrumentals, originals });

  const best = instrumentals[0];
  if (best && best.score >= INSTRUMENTAL_THRESHOLD) {
    await useInstrumental(best.video, token);
  } else if (originals[0]) {
    st().toast("No clean instrumental found — using the vocal remover on the original.", "info");
    await useVocalRemover(originals[0].video, token);
  } else {
    st().patchSong({ phase: "error", message: "No playable version of this song was found." });
  }
}

async function download(video: Video, token: number, label: string): Promise<string | null> {
  st().patchSong({ phase: "downloading", message: label, progress: 0 });
  try {
    return await api.ytDownload(video.id);
  } catch (e) {
    if (isCurrent(token)) st().patchSong({ phase: "error", message: errorText(e) });
    return null;
  }
}

async function useInstrumental(video: Video, token: number) {
  const path = await download(video, token, "Downloading instrumental…");
  if (!path || !isCurrent(token)) return;
  st().patchSong({ phase: "analyzing", message: "Analysing key and tempo…", instId: video.id });
  try {
    const info = await api.songLoad(path);
    if (!isCurrent(token)) return;
    st().patchSong({ phase: "ready", info, message: "" });
    applySongKey(info);
    void syncWithOriginal(token);
  } catch (e) {
    if (isCurrent(token) && errorText(e) !== "superseded") st().patchSong({ phase: "error", message: errorText(e) });
  }
}

async function useVocalRemover(video: Video, token: number) {
  const path = await download(video, token, "Downloading original…");
  if (!path || !isCurrent(token)) return;
  st().patchSong({ phase: "analyzing", message: "Removing lead vocal…" });
  try {
    const info = await api.songVocalRemoval(path);
    if (!isCurrent(token)) return;
    st().patchSong({ phase: "ready", info, origId: video.id, origPath: path, instId: null, sync: "synced" });
    applySongKey(info);
  } catch (e) {
    if (isCurrent(token) && errorText(e) !== "superseded") st().patchSong({ phase: "error", message: errorText(e) });
  }
}

/**
 * Lyrics are timed against the original recording; instrumentals often
 * start earlier or later. Download the original in the background and
 * cross-correlate the two to find the offset (also enables the guide vocal).
 */
async function syncWithOriginal(token: number) {
  const song = st().song;
  const orig = song?.originals[0];
  if (!song || !orig) {
    st().patchSong({ sync: "unavailable" });
    return;
  }
  try {
    const path = song.origPath ?? (await api.ytDownload(orig.video.id));
    if (!isCurrent(token)) return;
    const info = await api.songLoadGuide(path);
    if (!isCurrent(token)) return;
    const confident = (info.alignment?.confidence ?? 0) >= 0.2;
    st().patchSong({ info, origId: orig.video.id, origPath: path, sync: confident ? "synced" : "unsynced" });
    if (confident && Math.abs(info.offset) > 0.25) {
      st().toast(`Lyrics auto-synced (${info.offset > 0 ? "+" : ""}${info.offset.toFixed(2)} s)`, "success");
    }
  } catch (e) {
    if (isCurrent(token) && errorText(e) !== "superseded") st().patchSong({ sync: "unavailable" });
  }
}

export async function switchInstrumental(r: Ranked) {
  const token = ++loadToken;
  await api.pause().catch(() => {});
  st().patchSong({ sync: "pending", userOffset: 0 });
  await useInstrumental(r.video, token);
}

export async function switchToVocalRemover() {
  const song = st().song;
  const orig = song?.originals[0];
  if (!orig) return;
  const token = ++loadToken;
  await api.pause().catch(() => {});
  st().patchSong({ userOffset: 0 });
  await useVocalRemover(orig.video, token);
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
