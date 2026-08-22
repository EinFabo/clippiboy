import { useEffect, useState, type ReactNode } from "react";
import { cn } from "@/lib/cn";
import { SLIDE, SlidingIndicator } from "./SlidingIndicator";

export function Toggle({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: string;
  disabled?: boolean;
}) {
  return (
    <button
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        "relative h-6 w-11 shrink-0 rounded-pill transition-colors duration-200 ease-[var(--ease-out-soft)]",
        checked ? "bg-accent" : "bg-line-strong",
        disabled && "pointer-events-none opacity-40",
      )}
    >
      <span
        className={cn(
          "absolute top-0.5 left-0.5 h-5 w-5 rounded-pill bg-white shadow-sm",
          "transition-transform duration-200 ease-[var(--ease-out-soft)]",
          checked && "translate-x-5",
        )}
      />
    </button>
  );
}

/**
 * A row of options where one is picked, with a marker that travels to it.
 *
 * The whole row is one track with one frame around it, rather than a frame per
 * button: that is what lets the white pill slide from one option to the next
 * instead of switching off here and on over there.
 */
export function Segmented<K extends string>({
  value,
  options,
  onChange,
  disabled,
  className,
}: {
  /** `null` when nothing matches — the marker then stays away entirely. */
  value: K | null;
  options: Array<{ key: K; label: ReactNode }>;
  onChange: (key: K) => void;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "relative flex items-center gap-0.5 rounded-pill border border-line bg-elevated p-0.5",
        disabled && "pointer-events-none opacity-40",
        className,
      )}
    >
      {value !== null && (
        <SlidingIndicator activeKey={value} className="rounded-pill bg-white" />
      )}
      {options.map(({ key, label }) => (
        <button
          key={key}
          {...{ [SLIDE]: key }}
          onClick={() => onChange(key)}
          className={cn(
            // Positioned, so it stands above the marker instead of under it.
            "relative h-7 rounded-pill px-3 text-[13px] font-medium tabular-nums",
            "transition-colors duration-150 ease-[var(--ease-out-soft)]",
            value === key ? "text-black" : "text-ink-muted hover:text-ink",
          )}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

export function Slider({
  value,
  min,
  max,
  step = 1,
  onChange,
  label,
}: {
  value: number;
  min: number;
  max: number;
  step?: number;
  onChange: (v: number) => void;
  label?: string;
}) {
  const pct = ((value - min) / (max - min)) * 100;
  return (
    <input
      type="range"
      aria-label={label}
      value={value}
      min={min}
      max={max}
      step={step}
      onChange={(e) => onChange(Number(e.target.value))}
      className="h-1.5 w-full cursor-pointer appearance-none rounded-pill outline-none
        [&::-webkit-slider-thumb]:h-3.5 [&::-webkit-slider-thumb]:w-3.5
        [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:rounded-pill
        [&::-webkit-slider-thumb]:bg-white [&::-webkit-slider-thumb]:shadow"
      style={{
        background: `linear-gradient(90deg, var(--color-accent) ${pct}%, var(--color-line-strong) ${pct}%)`,
      }}
    />
  );
}

export function Select<T extends string>({
  value,
  options,
  onChange,
  label,
  disabled,
}: {
  value: T;
  options: Array<{ value: T; label: string }>;
  onChange: (v: T) => void;
  label?: string;
  disabled?: boolean;
}) {
  return (
    <select
      aria-label={label}
      value={value}
      disabled={disabled}
      onChange={(e) => onChange(e.target.value as T)}
      className={cn(
        `h-9 rounded-pill border border-line bg-elevated px-4 text-sm text-ink
        outline-none transition-colors hover:border-line-strong`,
        disabled && "pointer-events-none opacity-40",
      )}
    >
      {options.map((o) => (
        <option key={o.value} value={o.value} className="bg-elevated">
          {o.label}
        </option>
      ))}
    </select>
  );
}

/// Levels rise instantly and fall softly — otherwise the bar flickers at the 20
/// peak values a second the core delivers.
export function Meter({ level }: { level: number }) {
  const [shown, setShown] = useState(0);

  useEffect(() => {
    setShown((current) => (level > current ? level : current));
  }, [level]);

  useEffect(() => {
    const timer = setInterval(
      () => setShown((current) => (current < 0.005 ? 0 : current * 0.78)),
      60,
    );
    return () => clearInterval(timer);
  }, []);

  // Linear amplitude always looks like "nothing" on a bar: normal speech sits at
  // 0.03–0.1. Hence a dB scale from -60 to 0.
  const db = shown > 0 ? 20 * Math.log10(shown) : -Infinity;
  const pct = Math.min(100, Math.max(0, ((db + 60) / 60) * 100));

  return (
    <div className="h-1.5 w-full overflow-hidden rounded-pill bg-line">
      <div
        className="h-full rounded-pill transition-[width] duration-75 ease-linear"
        style={{
          width: `${pct}%`,
          background:
            pct > 92
              ? "var(--color-live)"
              : "linear-gradient(90deg, var(--color-ok), var(--color-accent-bright))",
        }}
      />
    </div>
  );
}
