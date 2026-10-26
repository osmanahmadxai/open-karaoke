import { useRef } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, errorText } from "../lib/api";
import { formatTime } from "../lib/music";
import { useStore } from "../lib/store";
import { Pause, Play, Rewind } from "./Icons";

export async function togglePlay() {
  const { playing, song } = useStore.getState();
  if (song?.phase !== "ready") return;
  if (playing) await api.pause();
  else await api.play();
}

export async function toggleRecord() {
  const s = useStore.getState();
  if (!s.song?.info) return;
  try {
    if (s.recording) {
      const path = await api.recordStop();
      if (path) {
        useStore.setState({ lastRecording: path });
        s.toast("Recording saved", "success", { label: "Show", run: () => revealItemInDir(path) });
      }
    } else {
      const stamp = new Date().toISOString().slice(0, 16).replace("T", " ").replace(":", "-");
      await api.recordStart(`${s.song.meta.artist} - ${s.song.meta.title} (${stamp})`);
      if (!s.playing) await api.play();
    }
  } catch (e) {
    s.toast(errorText(e), "error");
  }
}

function Meter({ value }: { value: number }) {
  // Peak (linear) → dB → 0..1 over a 48 dB range.
  const db = 20 * Math.log10(Math.max(value, 1e-5));
  const h = Math.max(0, Math.min(1, (db + 48) / 48));
  return (
    <div className="meter">
      <div style={{ height: `${h * 100}%` }} />
    </div>
  );
}

export function Transport() {
  const playing = useStore((s) => s.playing);
  const pos = useStore((s) => s.pos);
  const duration = useStore((s) => s.duration);
  const recording = useStore((s) => s.recording);
  const mic = useStore((s) => s.mic);
  const out = useStore((s) => s.out);
  const score = useStore((s) => s.score);
  const lines = useStore((s) => s.song?.lyrics.lines);
  const info = useStore((s) => s.song?.info);
  const userOffset = useStore((s) => s.song?.userOffset ?? 0);
  const ready = useStore((s) => s.ready);
  const separating = useStore((s) => !!s.song?.info?.separating);
  const seek = useRef<HTMLDivElement>(null);

  const ratio = duration > 0 ? Math.min(1, pos / duration) : 0;
  const tempo = info?.tempo ?? 1;
  const offset = userOffset;

  const onSeek = (e: React.PointerEvent) => {
    const el = seek.current;
    if (!el || duration <= 0) return;
    const go = (clientX: number) => {
      const r = el.getBoundingClientRect();
      const f = Math.max(0, Math.min(1, (clientX - r.left) / r.width));
      useStore.setState({ pos: f * duration });
      void api.seek(f * duration);
    };
    go(e.clientX);
    const move = (ev: PointerEvent) => go(ev.clientX);
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return (
    <div className="transport">
      <button className="icon-btn" title="Restart" onClick={() => api.seek(0)}>
        <Rewind />
      </button>
      <button className="play-btn" onClick={togglePlay} title="Play / pause (Space)">
        {playing ? <Pause /> : <Play style={{ marginLeft: 3 }} />}
      </button>
      <div className="timeline">
        <span>{formatTime(pos)}</span>
        <div className="seek" ref={seek} onPointerDown={onSeek}>
          <div className="rail">
            {separating && duration > 0 && <div className="ready-fill" style={{ width: `${Math.min(1, ready / duration) * 100}%` }} />}
            <div className="fill" style={{ width: `${ratio * 100}%` }} />
          </div>
          {duration > 0 &&
            lines?.map((l, i) => {
              const t = (l.start + offset) / tempo;
              if (t < 0 || t > duration) return null;
              const gap = i === 0 || l.start - lines[i - 1].end > 4;
              return gap ? <div key={i} className="mark" style={{ left: `${(t / duration) * 100}%` }} /> : null;
            })}
          <div className="knob" style={{ left: `${ratio * 100}%` }} />
        </div>
        <span>{formatTime(duration)}</span>
      </div>
      <div className="meters" title="Mic / output level">
        <Meter value={mic} />
        <Meter value={out} />
      </div>
      <div className="score-pill" title={`Pitch accuracy ${Math.round(score.accuracy)}%`}>
        <b>{score.points.toLocaleString()}</b>
        <span>{score.streak > 1 ? `${score.streak}× streak` : "score"}</span>
      </div>
      <button className={`rec-btn ${recording ? "on" : ""}`} onClick={toggleRecord} title="Record your performance">
        <i />
        {recording ? "Stop" : "Rec"}
      </button>
    </div>
  );
}
