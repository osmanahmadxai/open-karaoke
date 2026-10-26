import { useEffect, useRef, useState } from "react";
import { lineAt, type Line } from "../lib/lrc";
import { songTime, useStore } from "../lib/store";

const LEAD_IN = 3; // seconds of countdown before a line after a break

/** Song time (seconds in the original recording's timeline, where lyric stamps live). */
function lyricClock(): number {
  const s = useStore.getState().song;
  const tempo = s?.info?.tempo ?? 1;
  const offset = (s?.info?.offset ?? 0) + (s?.userOffset ?? 0);
  return songTime() * tempo - offset;
}

export function Lyrics() {
  const song = useStore((s) => s.song);
  const lyrics = song?.lyrics;

  if (!song) return null;
  if (lyrics?.lines) return <SyncedLyrics lines={lyrics.lines} />;

  return (
    <div className="lyrics">
      <div className="intro-title">{song.meta.title}</div>
      <div className="intro-artist">{song.meta.artist}</div>
      {lyrics?.status === "loading" && <div className="note">Looking up lyrics…</div>}
      {lyrics?.status === "ready" && lyrics.instrumental && <div className="note">This track is an instrumental.</div>}
      {lyrics?.status === "ready" && !lyrics.instrumental && lyrics.plain && <PlainLyrics text={lyrics.plain} />}
      {lyrics?.status === "missing" && <div className="note">No lyrics found for this song, but the stage is yours.</div>}
    </div>
  );
}

function SyncedLyrics({ lines }: { lines: Line[] }) {
  const meta = useStore((s) => s.song?.meta);
  const [idx, setIdx] = useState(-1);
  const [count, setCount] = useState(0);
  const idxRef = useRef(-1);
  const countRef = useRef(0);
  const wordEls = useRef<(HTMLSpanElement | null)[]>([]);

  useEffect(() => {
    let raf = 0;
    const loop = () => {
      const t = lyricClock();
      const i = lineAt(lines, t);
      if (i !== idxRef.current) {
        idxRef.current = i;
        setIdx(i);
      }
      const line = lines[i];
      if (line) {
        line.words.forEach((w, k) => {
          const el = wordEls.current[k];
          if (!el) return;
          const p = Math.max(0, Math.min(1, (t - w.start) / Math.max(0.05, w.end - w.start)));
          el.style.setProperty("--p", `${(p * 100).toFixed(1)}%`);
          el.classList.toggle("sung", p >= 1);
        });
      }
      // Countdown dots before the next line when there is a long break.
      const next = lines[i + 1] ?? (i < 0 ? lines[0] : undefined);
      const breakStart = line ? line.end : -Infinity;
      let c = 0;
      if (next && next.start - breakStart > LEAD_IN + 1.5) {
        const left = next.start - t;
        if (left > 0 && left <= LEAD_IN) c = Math.ceil(left);
      }
      if (c !== countRef.current) {
        countRef.current = c;
        setCount(c);
      }
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, [lines]);

  const cur = lines[idx];
  const inBreak = cur && lyricClock() > cur.end + 1.2 && lines[idx + 1] && lines[idx + 1].start - cur.end > LEAD_IN + 1.5;
  const showIntro = idx < 0;

  return (
    <div className="lyrics">
      <div className="countdown" style={{ visibility: count > 0 ? "visible" : "hidden" }}>
        {[3, 2, 1].map((n) => (
          <span key={n} className={count > 0 && count <= n ? "on" : ""} />
        ))}
      </div>
      {showIntro ? (
        <>
          <div className="intro-title">{meta?.title}</div>
          <div className="intro-artist">{meta?.artist}</div>
          {lines[0] && (
            <div className="lyric-line next" style={{ marginTop: 36 }}>
              {lines[0].text}
            </div>
          )}
        </>
      ) : (
        <>
          <div className="lyric-line prev">{lines[idx - 1]?.text ?? " "}</div>
          <div className={`lyric-line current ${inBreak ? "" : "active"}`} key={idx} style={inBreak ? { opacity: 0.35 } : undefined}>
            {cur?.words.map((w, k) => (
              <span key={k}>
                <span className="w" ref={(el) => {
                    wordEls.current[k] = el;
                  }}>
                  {w.text}
                </span>{" "}
              </span>
            ))}
          </div>
          <div className="lyric-line next">{lines[idx + 1]?.text ?? " "}</div>
          <div className="lyric-line next2">{lines[idx + 2]?.text ?? " "}</div>
        </>
      )}
    </div>
  );
}

/** Unsynced lyrics scroll in proportion to the song's progress. */
function PlainLyrics({ text }: { text: string }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let raf = 0;
    const loop = () => {
      const el = ref.current;
      const { duration } = useStore.getState();
      if (el && duration > 0) {
        const target = (songTime() / duration) * (el.scrollHeight - el.clientHeight * 0.6);
        el.scrollTop += (target - el.scrollTop) * 0.05;
      }
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, []);
  return (
    <div className="plain-lyrics" ref={ref}>
      {text}
    </div>
  );
}
