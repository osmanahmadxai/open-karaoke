import type { ReactNode } from "react";
import { useStore } from "../lib/store";

interface SliderProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  format?: (v: number) => string;
  onChange: (v: number) => void;
  left?: string;
  right?: string;
  /** Double-click resets to this value. */
  reset?: number;
}

export function Slider({ label, value, min, max, step = 0.01, format, onChange, left, right, reset }: SliderProps) {
  const pct = ((value - min) / (max - min)) * 100;
  return (
    <div className="field">
      <div className="field-head">
        <label>{label}</label>
        <span>{format ? format(value) : value.toFixed(2)}</span>
      </div>
      <input
        className="slider"
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        style={{ ["--v" as string]: `${pct}%` }}
        onChange={(e) => onChange(parseFloat(e.target.value))}
        onDoubleClick={() => reset !== undefined && onChange(reset)}
      />
      {(left || right) && (
        <div className="field-foot">
          <span>{left}</span>
          <span>{right}</span>
        </div>
      )}
    </div>
  );
}

/** A slider bound directly to an engine parameter. */
export function ParamSlider(props: Omit<SliderProps, "value" | "onChange"> & { name: string; scale?: number }) {
  const value = useStore((s) => s.params[props.name] ?? 0);
  const setParam = useStore((s) => s.setParam);
  const k = props.scale ?? 1;
  return <Slider {...props} value={value * k} onChange={(v) => setParam(props.name, v / k)} reset={props.reset !== undefined ? props.reset * k : undefined} />;
}

export function Toggle({ on, onChange, label, hint }: { on: boolean; onChange: (v: boolean) => void; label: ReactNode; hint?: string }) {
  return (
    <div className="toggle-row">
      <div>
        {label}
        {hint && <small>{hint}</small>}
      </div>
      <button className={`toggle ${on ? "on" : ""}`} role="switch" aria-checked={on} onClick={() => onChange(!on)} />
    </div>
  );
}

export function ParamToggle({ name, label, hint }: { name: string; label: ReactNode; hint?: string }) {
  const on = useStore((s) => (s.params[name] ?? 0) > 0.5);
  const setParam = useStore((s) => s.setParam);
  return <Toggle on={on} onChange={(v) => setParam(name, v ? 1 : 0)} label={label} hint={hint} />;
}

export function Select<T extends string | number>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: { label: string; value: T }[];
  onChange: (v: T) => void;
}) {
  return (
    <select
      className="select"
      value={String(value)}
      onChange={(e) => {
        const o = options.find((o) => String(o.value) === e.target.value);
        if (o) onChange(o.value);
      }}
    >
      {options.map((o) => (
        <option key={String(o.value)} value={String(o.value)}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

export function Group({ title, extra, children }: { title: string; extra?: ReactNode; children: ReactNode }) {
  return (
    <div className="group">
      <h4>
        {title}
        {extra}
      </h4>
      {children}
    </div>
  );
}

export function Segmented<T extends string | number>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: { label: string; value: T }[];
  onChange: (v: T) => void;
}) {
  return (
    <div className="seg">
      {options.map((o) => (
        <button key={String(o.value)} className={o.value === value ? "on" : ""} onClick={() => onChange(o.value)}>
          {o.label}
        </button>
      ))}
    </div>
  );
}

export const pct = (v: number) => `${Math.round(v * 100)}%`;
export const db = (v: number) => `${v > 0 ? "+" : ""}${v.toFixed(1)} dB`;
export const ms = (v: number) => `${Math.round(v)} ms`;
export const semis = (v: number) => `${v > 0 ? "+" : ""}${v.toFixed(v % 1 ? 1 : 0)} st`;
