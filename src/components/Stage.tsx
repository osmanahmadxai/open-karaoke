import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api } from "../lib/api";
import { closeSong, openSong } from "../lib/loader";
import { keyName } from "../lib/music";
import { useStore, type Phase } from "../lib/store";
import { Check, Circle, Play } from "./Icons";
import { Lyrics } from "./Lyrics";
import { MixPanel, SidebarTabs, SongPanel, TunePanel, VoicePanel } from "./Panels";
import { PitchGraph } from "./PitchGraph";
import { togglePlay, Transport } from "./Transport";

const STEPS: { phase: Phase; label: string }[] = [
  { phase: "searching", label: "Finding the best instrumental" },
  { phase: "downloading", label: "Downloading audio" },
  { phase: "analyzing", label: "Detecting key & preparing the stage" },
];

function LoadingCard() {
  const song = useStore((s) => s.song)!;
  const order = STEPS.findIndex((s) => s.phase === song.phase);
  return (
    <div className="overlay">
      <div className="loading-card">
        {song.meta.artwork && <img src={song.meta.artwork} alt="" />}
        <h2>{song.meta.title}</h2>
        <div className="a">{song.meta.artist}</div>
        {song.phase === "error" ? (
          <>
            <p className="err" style={{ margin: "0 0 18px" }}>
              {song.message}
            </p>
            <div className="row">
              <button className="btn" onClick={closeSong}>
                Back to search
              </button>
              <button className="btn primary" onClick={() => openSong(song.meta)}>
                Try again
              </button>
            </div>
          </>
        ) : (
          <div className="steps">
            {STEPS.map((s, i) => (
              <div key={s.phase} className={i < order ? "done" : i === order ? "on" : ""}>
                {i < order ? <Check style={{ color: "var(--ok)" }} /> : i === order ? <div className="spinner" /> : <Circle />}
                {s.label}
                {i === order && s.phase === "downloading" && song.progress > 0 && ` · ${Math.round(song.progress * 100)}%`}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function StartCard() {
  const song = useStore((s) => s.song)!;
  return (
    <div className="overlay" style={{ background: "rgba(11,10,16,0.35)" }}>
      <div className="start-card">
        <div className="ready">Ready</div>
        <h2>{song.meta.title}</h2>
        <div className="a">{song.meta.artist}</div>
        <button className="btn primary big" onClick={() => api.play()} autoFocus>
          <Play /> Start singing
        </button>
        <div className="hint">
          <span className="kbd">Space</span> play/pause · <span className="kbd">←</span>
          <span className="kbd">→</span> seek · <span className="kbd">[</span>
          <span className="kbd">]</span> lyric timing · <span className="kbd">F</span> fullscreen
        </div>
      </div>
    </div>
  );
}

function grade(acc: number) {
  if (acc >= 92) return "S";
  if (acc >= 82) return "A";
  if (acc >= 70) return "B";
  if (acc >= 55) return "C";
  return "D";
}

function ScoreCard() {
  const score = useStore((s) => s.score);
  const lastRecording = useStore((s) => s.lastRecording);
  const song = useStore((s) => s.song)!;
  const sang = score.sung > 5;
  return (
    <div className="overlay">
      <div className="scorecard">
        <div className="ready" style={{ color: "var(--muted)", letterSpacing: "0.18em", fontSize: 12, fontWeight: 700 }}>
          PERFORMANCE
        </div>
        <div className="grade">{sang ? grade(score.accuracy) : "–"}</div>
        <div className="stats">
          <div>
            <b>{sang ? `${Math.round(score.accuracy)}%` : "–"}</b>
            <span>pitch accuracy</span>
          </div>
          <div>
            <b>{score.points.toLocaleString()}</b>
            <span>points</span>
          </div>
          <div>
            <b>{Math.round(score.sung)}s</b>
            <span>sung</span>
          </div>
        </div>
        <div className="row">
          <button className="btn" onClick={closeSong}>
            New song
          </button>
          {lastRecording && (
            <button className="btn" onClick={() => revealItemInDir(lastRecording)}>
              Show recording
            </button>
          )}
          <button
            className="btn primary"
            onClick={() => {
              useStore.setState({ ended: false });
              void api.seek(0).then(() => api.play());
            }}
          >
            Sing {song.meta.title.length > 18 ? "it" : `"${song.meta.title}"`} again
          </button>
        </div>
      </div>
    </div>
  );
}

export function Stage() {
  const song = useStore((s) => s.song);
  const tab = useStore((s) => s.tab);
  const sidebar = useStore((s) => s.sidebar);
  const playing = useStore((s) => s.playing);
  const ended = useStore((s) => s.ended);
  const pos = useStore((s) => s.pos);
  const recording = useStore((s) => s.recording);
  const tuneOn = useStore((s) => s.params.tune_enabled > 0.5);
  const tuneKey = useStore((s) => s.params.tune_key);
  const tuneScale = useStore((s) => s.params.tune_scale);
  if (!song) return null;

  const loading = song.phase !== "ready";
  const fresh = song.phase === "ready" && !playing && pos < 0.25 && !ended;

  return (
    <div className={`stage ${sidebar ? "" : "no-sidebar"}`}>
      <div className="stage-bg">{song.meta.artwork && <img src={song.meta.artwork} alt="" />}</div>
      <div className="stage-main">
        <div className="song-head">
          {song.meta.artwork ? <img src={song.meta.artwork} alt="" /> : <div className="art-ph" />}
          <div className="meta">
            <div className="t">{song.meta.title}</div>
            <div className="a">{song.meta.artist}</div>
          </div>
          <div className="chips">
            {recording && (
              <span className="chip">
                <span className="dot live" />
                <b>Recording</b>
              </span>
            )}
            <span className="chip" title="Autotune target key">
              <span className={`dot ${tuneOn ? "ok" : ""}`} />
              Autotune <b>{tuneOn ? keyName(tuneKey, tuneScale) : "off"}</b>
            </span>
            {song.info && (song.info.semitones !== 0 || song.info.tempo !== 1) && (
              <span className="chip">
                {song.info.semitones !== 0 && (
                  <b>
                    {song.info.semitones > 0 ? "+" : ""}
                    {song.info.semitones} st
                  </b>
                )}
                {song.info.tempo !== 1 && <b>{Math.round(song.info.tempo * 100)}%</b>}
              </span>
            )}
            {song.info?.vocalRemoved && <span className="chip">Vocal remover</span>}
          </div>
        </div>
        <Lyrics />
        <PitchGraph />
        <Transport />
        {loading && <LoadingCard />}
        {fresh && <StartCard />}
        {ended && <ScoreCard />}
        {song.busy && (
          <div className="overlay" style={{ background: "rgba(11,10,16,0.3)" }}>
            <div className="loading-card" style={{ width: "auto", display: "flex", gap: 12, alignItems: "center", padding: "16px 22px" }}>
              <div className="spinner" />
              {song.busy}
            </div>
          </div>
        )}
      </div>
      <aside className="sidebar">
        <div className="tabs" style={{ display: "block" }}>
          <SidebarTabs />
        </div>
        {tab === "tune" && <TunePanel />}
        {tab === "voice" && <VoicePanel />}
        {tab === "mix" && <MixPanel />}
        {tab === "song" && <SongPanel />}
      </aside>
    </div>
  );
}

export { togglePlay };
