import { useEffect, useRef } from "react";
import { inScale, mod12, noteName } from "../lib/music";
import { pitchTrail, useStore } from "../lib/store";

const WINDOW = 6; // seconds of history
const READOUT = 128; // px reserved for the note readout

export function PitchGraph() {
  const canvas = useRef<HTMLCanvasElement>(null);
  const noteEl = useRef<HTMLDivElement>(null);
  const centsEl = useRef<HTMLDivElement>(null);
  const needle = useRef<HTMLElement>(null);

  useEffect(() => {
    let raf = 0;
    let center = 60;
    const draw = () => {
      raf = requestAnimationFrame(draw);
      const c = canvas.current;
      if (!c) return;
      const dpr = window.devicePixelRatio || 1;
      const w = c.clientWidth;
      const h = c.clientHeight;
      if (c.width !== Math.round(w * dpr) || c.height !== Math.round(h * dpr)) {
        c.width = Math.round(w * dpr);
        c.height = Math.round(h * dpr);
      }
      const ctx = c.getContext("2d")!;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);

      const { params } = useStore.getState();
      const tonic = Math.round(params.tune_key ?? 0);
      const scale = Math.round(params.tune_scale ?? 1);
      // Trail the newest data slightly so batched frames scroll in smoothly.
      const now = performance.now() / 1000 - 0.035;
      const plotW = w - READOUT;

      // Follow the singer's register smoothly.
      const recent = pitchTrail.slice(-120).filter((p) => p[4]);
      if (recent.length > 10) {
        const sorted = recent.map((p) => p[2]).sort((a, b) => a - b);
        const median = sorted[sorted.length >> 1];
        center += (median - center) * 0.04;
      }
      const span = 11;
      const lo = center - span;
      const hi = center + span;
      const y = (m: number) => h - ((m - lo) / (hi - lo)) * h;
      const x = (t: number) => plotW - ((now - t) / WINDOW) * plotW;

      // Note lanes: scale notes visible, tonic emphasised.
      ctx.font = "600 10px Inter Variable, system-ui";
      ctx.textBaseline = "middle";
      for (let n = Math.floor(lo); n <= Math.ceil(hi); n++) {
        const yy = Math.round(y(n)) + 0.5;
        const isTonic = mod12(n - tonic) === 0;
        if (inScale(n, tonic, scale)) {
          ctx.fillStyle = isTonic ? "rgba(255,61,127,0.08)" : "rgba(255,255,255,0.025)";
          const lane = Math.abs(y(n + 0.5) - y(n - 0.5));
          ctx.fillRect(0, yy - lane / 2, plotW, lane);
          ctx.strokeStyle = isTonic ? "rgba(255,61,127,0.35)" : "rgba(255,255,255,0.07)";
          ctx.beginPath();
          ctx.moveTo(0, yy);
          ctx.lineTo(plotW, yy);
          ctx.stroke();
          // Label every scale note when there's room, otherwise only the tonic and fifth.
          const fifth = mod12(n - tonic) === 7;
          if (lane >= 11 || isTonic || fifth) {
            ctx.fillStyle = isTonic ? "rgba(255,120,170,0.9)" : "rgba(255,255,255,0.28)";
            ctx.fillText(noteName(n), 14, yy);
          }
        }
      }

      const visible = pitchTrail.filter((p) => p[0] > now - WINDOW - 0.1);
      const path = (idx: 1 | 2) => {
        ctx.beginPath();
        let prev: number | null = null;
        for (const p of visible) {
          if (!p[4]) {
            prev = null;
            continue;
          }
          const px = x(p[0]);
          const py = y(p[idx]);
          if (prev === null || p[0] - prev > 0.06) ctx.moveTo(px, py);
          else ctx.lineTo(px, py);
          prev = p[0];
        }
      };

      // Raw voice underneath, corrected voice on top with a glow.
      ctx.lineJoin = "round";
      ctx.lineCap = "round";
      ctx.strokeStyle = "rgba(47,216,238,0.45)";
      ctx.lineWidth = 1.5;
      path(1);
      ctx.stroke();

      const grad = ctx.createLinearGradient(0, 0, plotW, 0);
      grad.addColorStop(0, "rgba(139,92,246,0.2)");
      grad.addColorStop(0.6, "#c13cf0");
      grad.addColorStop(1, "#ff3d7f");
      ctx.strokeStyle = grad;
      ctx.lineWidth = 3.5;
      ctx.shadowColor = "rgba(255,61,127,0.7)";
      ctx.shadowBlur = 12;
      path(2);
      ctx.stroke();
      ctx.shadowBlur = 0;

      // "Now" cursor.
      ctx.strokeStyle = "rgba(255,255,255,0.18)";
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(plotW - 0.5, 0);
      ctx.lineTo(plotW - 0.5, h);
      ctx.stroke();

      // Readout: latest frame within the last 150 ms.
      const last = pitchTrail[pitchTrail.length - 1];
      const live = last && last[4] && performance.now() / 1000 - last[0] < 0.2;
      if (noteEl.current && centsEl.current && needle.current) {
        if (live) {
          const n = noteName(last[3], false);
          const oct = Math.floor(last[3] / 12) - 1;
          noteEl.current.innerHTML = `${n}<small>${oct}</small>`;
          const cents = Math.round((last[1] - last[3]) * 100);
          centsEl.current.textContent = `${cents > 0 ? "+" : ""}${cents}¢ sung`;
          needle.current.style.left = `${Math.max(0, Math.min(100, 50 + cents))}%`;
          needle.current.style.opacity = "1";
        } else {
          noteEl.current.innerHTML = "–";
          centsEl.current.textContent = "sing to begin";
          needle.current.style.opacity = "0";
        }
      }
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, []);

  return (
    <div className="pitch">
      <canvas ref={canvas} />
      <div className="legend">
        <span>
          <i style={{ background: "rgba(47,216,238,0.8)" }} />
          Your voice
        </span>
        <span>
          <i style={{ background: "linear-gradient(90deg,#c13cf0,#ff3d7f)" }} />
          Tuned
        </span>
      </div>
      <div className="readout">
        <div className="nn" ref={noteEl}>
          –
        </div>
        <div className="tuner">
          <i ref={needle} style={{ left: "50%", opacity: 0 }} />
        </div>
        <div className="cents" ref={centsEl}>
          sing to begin
        </div>
      </div>
    </div>
  );
}
