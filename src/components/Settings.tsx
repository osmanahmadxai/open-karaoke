import { useEffect, useState } from "react";
import { api, errorText, type DeviceList } from "../lib/api";
import { syncAllParams, useStore } from "../lib/store";
import { Select } from "./Controls";

const BUFFERS = [
  { label: "64 samples (lowest latency)", value: 64 },
  { label: "128 samples", value: 128 },
  { label: "256 samples (recommended)", value: 256 },
  { label: "512 samples", value: 512 },
  { label: "1024 samples (safest)", value: 1024 },
];

export async function openAudio() {
  const { audioPrefs, toast } = useStore.getState();
  try {
    const status = await api.audioOpen(audioPrefs.input, audioPrefs.output, audioPrefs.buffer);
    useStore.setState({ audio: status });
    syncAllParams();
    if (status.inputError) toast(`Microphone unavailable: ${status.inputError}`, "error");
    return status;
  } catch (e) {
    toast(`Audio device error: ${errorText(e)}`, "error");
    return null;
  }
}

export function Settings() {
  const open = useStore((s) => s.settingsOpen);
  const prefs = useStore((s) => s.audioPrefs);
  const audio = useStore((s) => s.audio);
  const set = useStore((s) => s.set);
  const [devices, setDevices] = useState<DeviceList | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (open) api.audioDevices().then(setDevices).catch(() => setDevices(null));
  }, [open]);

  if (!open) return null;

  const apply = async (p: Partial<typeof prefs>) => {
    set({ audioPrefs: { ...prefs, ...p } });
    setBusy(true);
    await openAudio();
    setBusy(false);
  };

  const opt = (list: string[] | undefined, def: string | null | undefined) => [
    { label: `System default${def ? ` (${def})` : ""}`, value: "" },
    ...(list ?? []).map((d) => ({ label: d, value: d })),
  ];

  return (
    <div className="modal-bg" onMouseDown={(e) => e.target === e.currentTarget && set({ settingsOpen: false })}>
      <div className="modal">
        <h3>Audio setup</h3>
        <div className="sub">Headphones give the best experience: no feedback, and you hear the autotune exactly as recorded.</div>

        <label>Microphone</label>
        <Select value={prefs.input ?? ""} options={opt(devices?.inputs, devices?.defaultInput)} onChange={(v) => apply({ input: v || null })} />

        <label>Output</label>
        <Select value={prefs.output ?? ""} options={opt(devices?.outputs, devices?.defaultOutput)} onChange={(v) => apply({ output: v || null })} />

        <label>Buffer size</label>
        <Select value={prefs.buffer ?? 256} options={BUFFERS} onChange={(v) => apply({ buffer: v })} />
        <p className="help">Smaller buffers reduce the delay between singing and hearing yourself. Raise it if you hear crackles.</p>

        {audio && (
          <div className="status-box">
            <span>Output</span>
            <b>{audio.output}</b>
            <span>Microphone</span>
            <b className={audio.inputError ? "err" : ""}>{audio.inputError ?? audio.input ?? "None"}</b>
            <span>Sample rate</span>
            <b>{(audio.sampleRate / 1000).toFixed(1)} kHz</b>
            <span>Voice latency</span>
            <b>≈ {Math.round(audio.latencyMs)} ms</b>
          </div>
        )}

        <div className="row" style={{ justifyContent: "flex-end", marginTop: 20 }}>
          <button className="btn" disabled={busy} onClick={() => apply({})}>
            Reconnect
          </button>
          <button className="btn primary" onClick={() => set({ settingsOpen: false })}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
}
