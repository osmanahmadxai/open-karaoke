import { useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, errorText } from "../lib/api";
import { formatTime, keyName, NOTE_NAMES, SCALES } from "../lib/music";
import { useStore } from "../lib/store";
import { ParamSlider, Segmented, Select } from "./Controls";
import { Folder, Mic, Pause, Play } from "./Icons";
import { MixPanel, SidebarTabs, TunePanel, VoicePanel } from "./Panels";
import { PitchGraph } from "./PitchGraph";

const LAB_RECORDING = 1;
const LAB_PLAYING = 2;

async function run(fn: () => Promise<unknown>) {
  try {
    await fn();
  } catch (e) {
    useStore.getState().toast(errorText(e), "error");
  }
}

function TakeCard() {
  const lab = useStore((s) => s.lab);
  const toast = useStore((s) => s.toast);
  const [hasTake, setHasTake] = useState(false);
  const [looped, setLooped] = useState(true);
  const [ab, setAb] = useState<"tuned" | "original">("tuned");
  const [saving, setSaving] = useState(false);

  const recording = lab.state === LAB_RECORDING;
  const playing = lab.state === LAB_PLAYING;

  const record = () =>
    run(async () => {
      if (recording) {
        const secs = await api.labStopRecord();
        setHasTake(secs > 0.3);
        if (secs > 0.3) await api.labPlay(looped);
      } else {
        await api.labRecord();
      }
    });

  const play = () => run(() => (playing ? api.labStop() : api.labPlay(looped)));

  const save = () =>
    run(async () => {
      setSaving(true);
      try {
        const path = await api.labExport();
        toast("Take saved with your current settings", "success", { label: "Show", run: () => revealItemInDir(path) });
      } finally {
        setSaving(false);
      }
    });

  useEffect(() => {
    void api.labBypass(ab === "original");
  }, [ab]);

  // Recording auto-stops at the 30 s limit.
  useEffect(() => {
    if (recording && lab.len > 0 && lab.pos >= lab.len - 0.05) void record();
  }, [recording, lab.pos, lab.len]);

  const progress = lab.len > 0 ? Math.min(1, lab.pos / lab.len) : 0;

  return (
    <div className="lab-card">
      <div className="lab-card-head">
        <h3>Test take</h3>
        <span className="help" style={{ margin: 0 }}>
          {recording ? "Recording…" : hasTake ? "Loop it and tweak the settings on the right" : "Up to 30 seconds"}
        </span>
      </div>
      <div className="take-row">
        <button className={`rec-big ${recording ? "on" : ""}`} onClick={record} title={recording ? "Stop recording" : "Record a take"}>
          {recording ? <span className="sq" /> : <Mic />}
        </button>
        <button className="play-btn" onClick={play} disabled={!hasTake || recording} title="Play the take through your settings">
          {playing ? <Pause /> : <Play style={{ marginLeft: 3 }} />}
        </button>
        <div className="take-time">
          <div className="progress">
            <div style={{ width: `${(recording ? lab.pos / 30 : progress) * 100}%`, background: recording ? "var(--danger)" : undefined }} />
          </div>
          <div className="take-labels">
            <span>{formatTime(lab.pos)}</span>
            <span>{recording ? "0:30" : formatTime(hasTake ? lab.len : 0)}</span>
          </div>
        </div>
      </div>
      <div className="take-row" style={{ marginTop: 16 }}>
        <div style={{ flex: 1 }}>
          <Segmented
            value={ab}
            onChange={setAb}
            options={[
              { label: "With my settings", value: "tuned" },
              { label: "Original", value: "original" },
            ]}
          />
        </div>
        <button
          className="btn small"
          style={looped ? { borderColor: "rgba(193,60,240,.6)", background: "rgba(193,60,240,.16)" } : undefined}
          onClick={() => {
            setLooped(!looped);
            if (playing) void api.labPlay(!looped);
          }}
        >
          Loop {looped ? "on" : "off"}
        </button>
        <button className="btn small" disabled={!hasTake || saving} onClick={save}>
          {saving ? "Saving…" : "Save WAV"}
        </button>
        <button className="btn small" title="Open recordings folder" onClick={() => run(async () => revealItemInDir(await api.recordingsDir()))}>
          <Folder />
        </button>
      </div>
    </div>
  );
}

function DroneCard() {
  const params = useStore((s) => s.params);
  const setParam = useStore((s) => s.setParam);
  const on = params.drone > 0;
  return (
    <div className="lab-card">
      <div className="lab-card-head">
        <h3>Key drone</h3>
        <button className={`toggle ${on ? "on" : ""}`} role="switch" aria-checked={on} onClick={() => setParam("drone", on ? 0 : 0.6)} />
      </div>
      <p className="help" style={{ marginTop: 0, marginBottom: 14 }}>
        A soft {keyName(params.tune_key, params.tune_scale)} chord to sing against, so you can hear the autotune pull you into key.
      </p>
      <div className="two" style={{ marginBottom: 14 }}>
        <Select value={Math.round(params.tune_key)} options={NOTE_NAMES.map((n, i) => ({ label: n, value: i }))} onChange={(v) => setParam("tune_key", v)} />
        <Select value={Math.round(params.tune_scale)} options={SCALES.map((s, i) => ({ label: s.name, value: i }))} onChange={(v) => setParam("tune_scale", v)} />
      </div>
      <ParamSlider name="drone" label="Drone level" min={0} max={1} format={(v) => (v < 0.01 ? "Off" : `${Math.round(v * 100)}%`)} />
    </div>
  );
}

export function VoiceLab() {
  const tab = useStore((s) => s.tab);
  const sidebar = useStore((s) => s.sidebar);
  const input = useStore((s) => s.audio?.input);
  const inputError = useStore((s) => s.audio?.inputError);
  const set = useStore((s) => s.set);

  useEffect(() => {
    if (tab === "song") set({ tab: "tune" });
    return () => {
      // Leaving the lab: silence everything it started.
      void api.labStop();
      void api.labBypass(false);
      useStore.getState().setParam("drone", 0);
    };
  }, []);

  return (
    <div className={`stage ${sidebar ? "" : "no-sidebar"}`}>
      <div className="stage-bg lab-bg" />
      <div className="stage-main lab-main">
        <div className="lab-head">
          <h1>
            Voice <em>Lab</em>
          </h1>
          <p>
            Build your sound before you sing. Record a short take, loop it, and shape the autotune and effects until it sounds right, then
            save it under <b>My presets</b> to use in any song.
          </p>
          <div className="chips" style={{ justifyContent: "flex-start" }}>
            <span className="chip">
              <span className={`dot ${inputError ? "warn" : "ok"}`} />
              Mic <b>{inputError ? "unavailable" : (input ?? "—")}</b>
            </span>
            <button className="chip" onClick={() => set({ settingsOpen: true })}>
              Change audio devices
            </button>
          </div>
        </div>
        <PitchGraph />
        <div className="lab-cards">
          <TakeCard />
          <DroneCard />
        </div>
      </div>
      <aside className="sidebar">
        <div className="tabs" style={{ display: "block" }}>
          <SidebarTabs lab />
        </div>
        {tab === "tune" && <TunePanel />}
        {tab === "voice" && <VoicePanel />}
        {tab === "mix" && <MixPanel />}
      </aside>
    </div>
  );
}
