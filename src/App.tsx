import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, onDownloadProgress, onTick } from "./lib/api";
import { closeSong } from "./lib/loader";
import { handleTick, useStore } from "./lib/store";
import { Back, Expand, Mic, Panel, Settings as SettingsIcon } from "./components/Icons";
import { Search } from "./components/Search";
import { openAudio, Settings } from "./components/Settings";
import { Setup } from "./components/Setup";
import { Stage } from "./components/Stage";
import { togglePlay } from "./components/Transport";
import { VoiceLab } from "./components/VoiceLab";

const isMac = navigator.userAgent.includes("Mac");

/** macOS keeps native traffic lights; Windows and Linux get matching custom controls. */
function WindowControls() {
  const w = getCurrentWindow();
  return (
    <div className="win-controls">
      <button onClick={() => w.minimize()} title="Minimize">
        <svg viewBox="0 0 12 12" stroke="currentColor" strokeWidth="1.2">
          <path d="M2 6h8" />
        </svg>
      </button>
      <button onClick={() => w.toggleMaximize()} title="Maximize">
        <svg viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.2">
          <rect x="2" y="2" width="8" height="8" rx="1.5" />
        </svg>
      </button>
      <button className="close" onClick={() => w.close()} title="Close">
        <svg viewBox="0 0 12 12" stroke="currentColor" strokeWidth="1.2">
          <path d="m2.5 2.5 7 7m0-7-7 7" />
        </svg>
      </button>
    </div>
  );
}

async function toggleFullscreen() {
  const w = getCurrentWindow();
  await w.setFullscreen(!(await w.isFullscreen()));
}

function TitleBar() {
  const view = useStore((s) => s.view);
  const sidebar = useStore((s) => s.sidebar);
  const set = useStore((s) => s.set);
  return (
    <div className={`titlebar ${isMac ? "" : "win"}`} data-tauri-drag-region>
      {view === "stage" || view === "lab" ? (
        <button className="icon-btn" onClick={view === "lab" ? () => set({ view: "search" }) : closeSong} title="Back to search (Esc)">
          <Back />
        </button>
      ) : (
        <div className="brand" data-tauri-drag-region>
          <div className="brand-dot" />
          Croonly
        </div>
      )}
      <div className="spacer" data-tauri-drag-region />
      {view === "search" && (
        <button className="btn small" onClick={() => set({ view: "lab" })} title="Test your sound">
          <Mic /> Voice Lab
        </button>
      )}
      {(view === "stage" || view === "lab") && (
        <>
          <button className="icon-btn" onClick={toggleFullscreen} title="Fullscreen (F)">
            <Expand />
          </button>
          <button className={`icon-btn ${sidebar ? "active" : ""}`} onClick={() => set({ sidebar: !sidebar })} title="Toggle controls (Tab)">
            <Panel />
          </button>
        </>
      )}
      {view !== "setup" && view !== "boot" && (
        <button className="icon-btn" onClick={() => set({ settingsOpen: true })} title="Audio setup">
          <SettingsIcon />
        </button>
      )}
      {!isMac && <WindowControls />}
    </div>
  );
}

function Toasts() {
  const toasts = useStore((s) => s.toasts);
  const dismiss = useStore((s) => s.dismiss);
  return (
    <div className="toasts">
      {toasts.map((t) => (
        <div key={t.id} className={`toast ${t.kind}`}>
          {t.text}
          {t.action && (
            <button
              onClick={() => {
                t.action!.run();
                dismiss(t.id);
              }}
            >
              {t.action.label}
            </button>
          )}
        </div>
      ))}
    </div>
  );
}

function useShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      if (target.tagName === "INPUT" && (target as HTMLInputElement).type !== "range") return;
      if (target.tagName === "SELECT") return;
      const s = useStore.getState();
      if (s.view === "lab") {
        if (e.key === "Escape") {
          if (s.settingsOpen) s.set({ settingsOpen: false });
          else s.set({ view: "search" });
        }
        return;
      }
      if (s.view !== "stage" || !s.song) return;
      const nudge = (d: number) => {
        const song = useStore.getState().song!;
        const userOffset = Math.round((song.userOffset + d) * 100) / 100;
        s.patchSong({ userOffset });
        void api.songSetOffset((song.info?.offset ?? 0) + userOffset);
      };
      switch (e.key) {
        case " ":
          e.preventDefault();
          void togglePlay();
          break;
        case "ArrowLeft":
          e.preventDefault();
          void api.seek(Math.max(0, s.pos - 5));
          break;
        case "ArrowRight":
          e.preventDefault();
          void api.seek(s.pos + 5);
          break;
        case "[":
          nudge(-0.1);
          break;
        case "]":
          nudge(0.1);
          break;
        case "f":
        case "F":
          void toggleFullscreen();
          break;
        case "Tab":
          e.preventDefault();
          s.set({ sidebar: !s.sidebar });
          break;
        case "Escape":
          if (s.settingsOpen) s.set({ settingsOpen: false });
          else closeSong();
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

export default function App() {
  const view = useStore((s) => s.view);
  useShortcuts();

  useEffect(() => {
    const tick = onTick(handleTick);
    const dl = onDownloadProgress((p) => {
      const song = useStore.getState().song;
      if (song?.phase === "downloading") useStore.getState().patchSong({ progress: p.fraction });
    });
    (async () => {
      const tools = await api.toolsStatus().catch(() => null);
      useStore.setState({ tools, view: tools?.ready ? "search" : "setup" });
      await openAudio();
    })();
    return () => {
      void tick.then((f) => f());
      void dl.then((f) => f());
    };
  }, []);

  return (
    <div className="app">
      <TitleBar />
      {view === "setup" && <Setup />}
      {view === "search" && <Search />}
      {view === "stage" && <Stage />}
      {view === "lab" && <VoiceLab />}
      <Settings />
      <Toasts />
    </div>
  );
}
