import { useEffect, useRef, useState } from "react";
import { switchSource, switchToVocalRemover, transform } from "../lib/loader";
import { formatTime, HARMONIES, keyName, NOTE_NAMES, SCALES } from "../lib/music";
import { FX_PRESETS, SOUND_KEYS, TUNE_PRESETS, type Preset } from "../lib/presets";
import { useStore, type Tab } from "../lib/store";
import { db, Group, ms, ParamSlider, ParamToggle, pct, Segmented, Select, semis, Slider, Toggle } from "./Controls";

function matches(values: Record<string, number>, params: Record<string, number>) {
  return Object.entries(values).every(([k, v]) => Math.abs((params[k] ?? 0) - v) < 1e-3);
}

function PresetRow({ presets }: { presets: Preset[] }) {
  const params = useStore((s) => s.params);
  const setParams = useStore((s) => s.setParams);
  return (
    <div className="presets">
      {presets.map((p) => (
        <button key={p.name} title={p.hint} className={`preset ${matches(p.values, params) ? "on" : ""}`} onClick={() => setParams(p.values)}>
          {p.name}
        </button>
      ))}
    </div>
  );
}

/** Save the current autotune + effects combination under a name. */
function MyPresets() {
  const myPresets = useStore((s) => s.myPresets);
  const params = useStore((s) => s.params);
  const setParams = useStore((s) => s.setParams);
  const savePreset = useStore((s) => s.savePreset);
  const deletePreset = useStore((s) => s.deletePreset);
  const toast = useStore((s) => s.toast);
  const [naming, setNaming] = useState(false);
  const [name, setName] = useState("");

  const save = () => {
    const n = name.trim();
    if (!n) return;
    const values: Record<string, number> = {};
    for (const k of SOUND_KEYS) values[k] = params[k];
    savePreset(n, values);
    setNaming(false);
    setName("");
    toast(`Saved "${n}"`, "success");
  };

  return (
    <Group
      title="My presets"
      extra={
        !naming && (
          <button className="badge muted" style={{ cursor: "pointer" }} onClick={() => setNaming(true)}>
            + Save current
          </button>
        )
      }
    >
      {naming && (
        <div className="name-row">
          <input
            autoFocus
            className="text-input"
            placeholder="Name this sound…"
            value={name}
            maxLength={32}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") save();
              if (e.key === "Escape") setNaming(false);
            }}
          />
          <button className="btn small primary" onClick={save} disabled={!name.trim()}>
            Save
          </button>
        </div>
      )}
      {myPresets.length ? (
        <div className="presets">
          {myPresets.map((p) => (
            <span key={p.name} className={`preset my ${matches(p.values, params) ? "on" : ""}`}>
              <button onClick={() => setParams(p.values)} title="Apply">
                {p.name}
              </button>
              <button className="x" title="Delete" onClick={() => deletePreset(p.name)}>
                ×
              </button>
            </span>
          ))}
        </div>
      ) : (
        !naming && <p className="help" style={{ marginTop: 0 }}>Shape your autotune and effects, then save the combination here.</p>
      )}
    </Group>
  );
}

export function TunePanel() {
  const params = useStore((s) => s.params);
  const setParam = useStore((s) => s.setParam);
  const setParams = useStore((s) => s.setParams);
  const song = useStore((s) => s.song);
  const detected = song?.info?.key;
  const keyAuto = song?.keyAuto ?? true;
  const patchSong = useStore((s) => s.patchSong);

  const setKey = (p: Record<string, number>) => {
    patchSong({ keyAuto: false });
    setParams(p);
  };

  return (
    <div className="panel">
      <Group title="Autotune">
        <ParamToggle name="tune_enabled" label="Pitch correction" hint="Snaps your voice to the song's key" />
        <PresetRow presets={TUNE_PRESETS} />
      </Group>
      <MyPresets />

      <Group
        title="Key"
        extra={
          detected && (
            <span className={`badge ${keyAuto ? "" : "muted"}`} title={`Detected with ${Math.round(detected.confidence * 100)}% confidence`}>
              {keyAuto ? "Auto" : "Manual"}
            </span>
          )
        }
      >
        <div className="two">
          <Select value={Math.round(params.tune_key)} options={NOTE_NAMES.map((n, i) => ({ label: n, value: i }))} onChange={(v) => setKey({ tune_key: v })} />
          <Select value={Math.round(params.tune_scale)} options={SCALES.map((s, i) => ({ label: s.name, value: i }))} onChange={(v) => setKey({ tune_scale: v })} />
        </div>
        {detected && !keyAuto && (
          <p className="help">
            Song detected in {keyName(detected.tonic + Math.round(song?.info?.semitones ?? 0), detected.minor ? 2 : 1)}.{" "}
            <button
              style={{ color: "var(--accent)", fontWeight: 600 }}
              onClick={() => {
                patchSong({ keyAuto: true });
                setParams({ tune_key: ((detected.tonic + Math.round(song?.info?.semitones ?? 0)) % 12 + 12) % 12, tune_scale: detected.minor ? 2 : 1 });
              }}
            >
              Use it
            </button>
          </p>
        )}
      </Group>

      <Group title="Correction">
        <ParamSlider name="tune_retune_ms" label="Retune speed" min={0} max={400} step={1} format={(v) => (v < 1 ? "Instant" : ms(v))} left="Robotic" right="Natural" reset={40} />
        <ParamSlider name="tune_amount" label="Amount" min={0} max={1} format={pct} reset={1} />
        <ParamSlider name="tune_humanize" label="Humanize" min={0} max={1} format={pct} reset={0.3} left="Tight" right="Loose sustains" />
        <ParamSlider name="tune_flex" label="Flex-tune" min={0} max={50} step={1} format={(v) => (v < 1 ? "Off" : `${Math.round(v)}¢`)} left="Correct all" right="Keep expression" reset={0} />
        <ParamSlider name="tune_sensitivity" label="Detection threshold" min={0.5} max={0.95} format={pct} reset={0.8} />
      </Group>

      <Group title="Vibrato">
        <ParamSlider name="tune_vibrato_depth" label="Depth" min={0} max={80} step={1} format={(v) => (v < 1 ? "Off" : `${Math.round(v)}¢`)} reset={0} />
        <ParamSlider name="tune_vibrato_rate" label="Rate" min={3} max={8} step={0.1} format={(v) => `${v.toFixed(1)} Hz`} reset={5.5} />
      </Group>

      <Group title="Voice character">
        <ParamSlider name="tune_transpose" label="Pitch shift" min={-12} max={12} step={1} format={semis} reset={0} />
        <ParamSlider name="tune_formant" label="Formant" min={-8} max={8} step={0.5} format={semis} left="Deeper" right="Brighter" reset={0} />
        <ParamSlider name="tune_mix" label="Wet / dry" min={0} max={1} format={pct} reset={1} />
      </Group>

      <Group title="Harmonizer">
        <div className="two" style={{ marginBottom: 14 }}>
          <Select value={Math.round(params.harmony_a)} options={HARMONIES} onChange={(v) => setParam("harmony_a", v)} />
          <Select value={Math.round(params.harmony_b)} options={HARMONIES} onChange={(v) => setParam("harmony_b", v)} />
        </div>
        <ParamSlider name="harmony_level" label="Harmony level" min={0} max={1} format={pct} reset={0.5} />
        <ParamSlider name="harmony_spread" label="Stereo spread" min={0} max={1} format={pct} reset={0.7} />
        <ParamSlider name="doubler" label="Doubler" min={0} max={1} format={(v) => (v < 0.01 ? "Off" : pct(v))} reset={0} />
        <p className="help">Harmonies follow the scale, so a "3rd above" is always a note that belongs to the key.</p>
      </Group>
    </div>
  );
}

export function VoicePanel() {
  return (
    <div className="panel">
      <Group title="Vocal effects">
        <PresetRow presets={FX_PRESETS} />
      </Group>
      <MyPresets />
      <Group title="Input">
        <ParamSlider name="vocal_gain_db" label="Mic gain" min={-12} max={24} step={0.5} format={db} reset={0} />
        <ParamSlider name="gate_db" label="Noise gate" min={-80} max={-20} step={1} format={(v) => (v <= -79 ? "Off" : `${Math.round(v)} dB`)} reset={-50} />
        <ParamSlider name="hpf_hz" label="Low cut" min={20} max={400} step={1} format={(v) => `${Math.round(v)} Hz`} reset={90} />
      </Group>
      <Group title="Tone">
        <ParamSlider name="eq_low_db" label="Warmth" min={-12} max={12} step={0.5} format={db} reset={0} />
        <ParamSlider name="eq_presence_db" label="Presence" min={-12} max={12} step={0.5} format={db} reset={2} />
        <ParamSlider name="eq_air_db" label="Air" min={-12} max={12} step={0.5} format={db} reset={2} />
        <ParamSlider name="deesser" label="De-esser" min={0} max={1} format={pct} reset={0.4} />
        <ParamSlider name="comp" label="Compression" min={0} max={1} format={pct} reset={0.5} />
      </Group>
      <Group title="Echo">
        <ParamSlider name="delay_mix" label="Level" min={0} max={0.8} format={pct} reset={0} />
        <ParamSlider name="delay_ms" label="Time" min={60} max={1200} step={5} format={ms} reset={320} />
        <ParamSlider name="delay_feedback" label="Repeats" min={0} max={0.9} format={pct} reset={0.3} />
      </Group>
      <Group title="Reverb">
        <ParamSlider name="reverb_mix" label="Level" min={0} max={0.8} format={pct} reset={0.18} />
        <ParamSlider name="reverb_size" label="Room size" min={0} max={1} format={pct} reset={0.7} />
        <ParamSlider name="reverb_damp" label="Damping" min={0} max={1} format={pct} reset={0.4} />
      </Group>
    </div>
  );
}

export function MixPanel() {
  const hasGuide = useStore((s) => s.song?.info?.hasGuide ?? false);
  const vocalRemoved = useStore((s) => s.song?.info?.vocalRemoved ?? false);
  const latency = useStore((s) => s.params.record_latency_ms);
  const setParam = useStore((s) => s.setParam);
  return (
    <div className="panel">
      <Group title="Levels">
        <ParamSlider name="instrumental" label="Instrumental" min={0} max={1.2} format={pct} reset={0.85} />
        <ParamSlider name="vocal" label="Your voice" min={0} max={1.5} format={pct} reset={0.9} />
        {hasGuide && !vocalRemoved && (
          <ParamSlider name="guide" label="Original singer (guide)" min={0} max={1} format={(v) => (v < 0.01 ? "Off" : pct(v))} reset={0} />
        )}
        <ParamSlider name="master" label="Master" min={0} max={1.2} format={pct} reset={0.9} />
      </Group>
      <Group title="Monitoring">
        <ParamToggle name="monitor" label="Hear myself" hint="Use headphones to avoid feedback through speakers" />
      </Group>
      <Group title="Recording">
        <Toggle
          on={latency < 0}
          onChange={(auto) => setParam("record_latency_ms", auto ? -1 : 40)}
          label="Automatic latency compensation"
          hint="Lines your recorded voice up with the music"
        />
        {latency >= 0 && <ParamSlider name="record_latency_ms" label="Manual offset" min={0} max={300} step={1} format={ms} />}
      </Group>
    </div>
  );
}

export function SongPanel() {
  const song = useStore((s) => s.song);
  const patchSong = useStore((s) => s.patchSong);
  const info = song?.info;
  const [semi, setSemi] = useState(0);
  const [tempo, setTempo] = useState(1);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => {
    setSemi(info?.semitones ?? 0);
    setTempo(info?.tempo ?? 1);
  }, [info?.semitones, info?.tempo]);

  if (!song) return null;

  const separating = !!info?.separating;
  const commit = (s: number, t: number) => {
    setSemi(s);
    setTempo(t);
    clearTimeout(timer.current);
    timer.current = window.setTimeout(() => transform(s, t), 450);
  };

  const nudge = (d: number) => patchSong({ userOffset: Math.round((song.userOffset + d) * 100) / 100 });
  const busyLoading = song.phase === "searching" || song.phase === "downloading";

  return (
    <div className="panel">
      <Group
        title="Key of the music"
        extra={song.busy ? <span className="badge muted">{song.busy}</span> : separating && <span className="badge muted">After separation</span>}
      >
        <div className="stepper" style={separating ? { opacity: 0.45, pointerEvents: "none" } : undefined}>
          <button onClick={() => commit(Math.max(-6, semi - 1), tempo)}>−</button>
          <div className="val">{semis(semi)}</div>
          <button onClick={() => commit(Math.min(6, semi + 1), tempo)}>+</button>
        </div>
        <p className="help">
          {info?.key ? `Original key: ${keyName(info.key.tonic, info.key.minor ? 2 : 1)}. ` : ""}Move the song into your vocal range. The
          autotune key follows automatically.
          {separating && " Available as soon as the AI finishes separating this song."}
        </p>
      </Group>

      <Group title="Tempo">
        <div style={separating ? { opacity: 0.45, pointerEvents: "none" } : undefined}>
          <Slider label="Speed" value={tempo} min={0.7} max={1.3} step={0.05} format={pct} reset={1} onChange={(v) => commit(semi, v)} left="Slower" right="Faster" />
        </div>
      </Group>

      <Group title="Lyrics timing" extra={!info?.vocalRemoved && info && <span className="badge">Synced to original</span>}>
        <div className="stepper">
          <button onClick={() => nudge(-0.1)} title="Lyrics earlier ( [ )">−</button>
          <div className="val">{`${song.userOffset >= 0 ? "+" : ""}${song.userOffset.toFixed(2)} s`}</div>
          <button onClick={() => nudge(0.1)} title="Lyrics later ( ] )">+</button>
        </div>
        <p className="help">
          The music is separated from the same recording the lyrics are timed to, so they line up on their own. Use{" "}
          <span className="kbd">[</span> and <span className="kbd">]</span> to fine-tune.
          {song.userOffset !== 0 && (
            <>
              {" "}
              <button style={{ color: "var(--accent)", fontWeight: 600 }} onClick={() => nudge(-song.userOffset)}>
                Reset
              </button>
            </>
          )}
        </p>
      </Group>

      <Group title="Recording">
        {song.originals
          .filter((r, i) => i === 0 || r.score >= 0)
          .slice(0, 6)
          .map((r) => (
            <button
              key={r.video.id}
              className={`source ${song.sourceId === r.video.id && !info?.vocalRemoved ? "on" : ""}`}
              onClick={() => (song.sourceId !== r.video.id || info?.vocalRemoved) && switchSource(r)}
              disabled={busyLoading}
            >
              <img className="thumb" src={`https://i.ytimg.com/vi/${r.video.id}/mqdefault.jpg`} alt="" loading="lazy" />
              <div style={{ minWidth: 0 }}>
                <div className="st">{r.video.title}</div>
                <div className="sc">
                  {r.video.channel} · {formatTime(r.video.duration ?? 0)}
                </div>
              </div>
            </button>
          ))}
        {song.sourcePath && (
          <button className={`source ${info?.vocalRemoved ? "on" : ""}`} onClick={() => !info?.vocalRemoved && switchToVocalRemover()} disabled={busyLoading}>
            <div className="thumb" style={{ display: "grid", placeItems: "center", fontSize: 16 }}>
              ✂︎
            </div>
            <div>
              <div className="st">Basic vocal remover</div>
              <div className="sc">Instant, lower quality. Use if the AI can't run.</div>
            </div>
          </button>
        )}
        {!song.originals.length && <p className="help">Searching for the original recording…</p>}
        <p className="help">The AI separates the vocals from whichever recording you pick. Choose another if this one has a long intro or is a live version.</p>
      </Group>
    </div>
  );
}

export function SidebarTabs({ lab = false }: { lab?: boolean }) {
  const tab = useStore((s) => s.tab);
  const set = useStore((s) => s.set);
  const options: { label: string; value: Tab }[] = [
    { label: "Tune", value: "tune" },
    { label: "Voice", value: "voice" },
    { label: "Mix", value: "mix" },
  ];
  if (!lab) options.push({ label: "Song", value: "song" });
  return <Segmented value={tab} onChange={(t) => set({ tab: t })} options={options} />;
}
