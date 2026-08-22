import { useRef, useState, type PointerEvent as ReactPointerEvent } from "react";

import { cn } from "@/lib/cn";

/**
 * Hex to the three channels. Anything unreadable comes back black rather than
 * throwing — this sits behind a text field somebody is still typing in.
 */
export function toRgb(hex: string): [number, number, number] {
  const clean = hex.replace("#", "");
  const full =
    clean.length === 3
      ? clean
          .split("")
          .map((c) => c + c)
          .join("")
      : clean;
  const value = Number.parseInt(full.slice(0, 6), 16);
  if (!Number.isFinite(value)) return [0, 0, 0];
  return [(value >> 16) & 255, (value >> 8) & 255, value & 255];
}

export function toHex(r: number, g: number, b: number): string {
  const part = (value: number) =>
    Math.max(0, Math.min(255, Math.round(value)))
      .toString(16)
      .padStart(2, "0");
  return `#${part(r)}${part(g)}${part(b)}`;
}

/** Hue in degrees, saturation and value from 0 to 1. */
function toHsv(r: number, g: number, b: number): [number, number, number] {
  const [red, green, blue] = [r / 255, g / 255, b / 255];
  const max = Math.max(red, green, blue);
  const min = Math.min(red, green, blue);
  const span = max - min;

  let hue = 0;
  if (span !== 0) {
    if (max === red) hue = ((green - blue) / span) % 6;
    else if (max === green) hue = (blue - red) / span + 2;
    else hue = (red - green) / span + 4;
  }
  hue = (hue * 60 + 360) % 360;
  return [hue, max === 0 ? 0 : span / max, max];
}

function fromHsv(hue: number, saturation: number, value: number): string {
  const chroma = value * saturation;
  const second = chroma * (1 - Math.abs(((hue / 60) % 2) - 1));
  const rest = value - chroma;
  const [r, g, b] =
    hue < 60
      ? [chroma, second, 0]
      : hue < 120
        ? [second, chroma, 0]
        : hue < 180
          ? [0, chroma, second]
          : hue < 240
            ? [0, second, chroma]
            : hue < 300
              ? [second, 0, chroma]
              : [chroma, 0, second];
  return toHex((r + rest) * 255, (g + rest) * 255, (b + rest) * 255);
}

/**
 * A colour, picked from a handful of good ones or mixed by hand.
 *
 * The swatches carry the weight: nine times out of ten the answer is "red" and
 * that should be one click. The picker underneath unfolds in place rather than
 * floating — the column it sits in scrolls, and a popover would be cut off at
 * its edge.
 */
export function ColorPicker({
  value,
  swatches,
  onChange,
}: {
  value: string;
  swatches: string[];
  onChange: (hex: string) => void;
}) {
  const [open, setOpen] = useState(false);
  /**
   * What stands in the hex field while it is being typed in.
   *
   * A field controlled by `value` alone cannot be typed in at all: a keystroke
   * that does not complete six digits changes nothing, and the next render puts
   * the old colour straight back — so the caret never gets past the first one.
   * Hence a text of its own, handed on only once it really is a colour. `null`
   * means the field is showing `value` again.
   */
  const [typed, setTyped] = useState<string | null>(null);
  const custom = !swatches.includes(value.toLowerCase());
  const [r, g, b] = toRgb(value);
  const [hue, saturation, brightness] = toHsv(r, g, b);
  const field = useRef<HTMLDivElement>(null);

  const pickFromField = (event: ReactPointerEvent) => {
    const box = field.current?.getBoundingClientRect();
    if (!box) return;
    const x = Math.min(Math.max((event.clientX - box.left) / box.width, 0), 1);
    const y = Math.min(Math.max((event.clientY - box.top) / box.height, 0), 1);
    onChange(fromHsv(hue, x, 1 - y));
  };

  const channel = (at: number, next: number) => {
    const rgb: [number, number, number] = [r, g, b];
    rgb[at] = next;
    onChange(toHex(...rgb));
  };

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-1.5">
        {swatches.map((swatch) => (
          <button
            key={swatch}
            type="button"
            aria-label={`Colour ${swatch}`}
            aria-pressed={value.toLowerCase() === swatch}
            onClick={() => onChange(swatch)}
            style={{ background: swatch }}
            className={cn(
              "h-6 w-6 rounded-pill border-2 transition-transform duration-150",
              "ease-[var(--ease-out-soft)] hover:scale-110",
              value.toLowerCase() === swatch ? "border-white" : "border-line",
            )}
          />
        ))}
        {/* The way to everything else. It wears the mixed colour once there is
            one, so the row always shows what is actually set. */}
        <button
          type="button"
          aria-label="Mix a colour"
          aria-expanded={open}
          onClick={() => setOpen((was) => !was)}
          className={cn(
            "ml-auto grid h-6 w-6 place-items-center rounded-pill border-2",
            "transition-transform duration-150 ease-[var(--ease-out-soft)] hover:scale-110",
            custom ? "border-white" : "border-line",
          )}
          style={{
            background: custom
              ? value
              : "conic-gradient(#ef4444, #f59e0b, #34d399, #38bdf8, #a78bfa, #ef4444)",
          }}
        >
          {!custom && <span className="h-2 w-2 rounded-pill bg-black/60" />}
        </button>
      </div>

      {open && (
        <div className="space-y-2 rounded-inner border border-line bg-base p-2">
          <div
            ref={field}
            onPointerDown={(event) => {
              (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
              pickFromField(event);
            }}
            onPointerMove={(event) => {
              if (event.buttons === 1) pickFromField(event);
            }}
            className="relative h-24 w-full cursor-crosshair rounded-[8px] touch-none"
            style={{
              background: `linear-gradient(to top, #000, transparent),
                linear-gradient(to right, #fff, hsl(${hue} 100% 50%))`,
            }}
          >
            <span
              className="pointer-events-none absolute h-3 w-3 -translate-x-1/2 -translate-y-1/2
                rounded-pill border-2 border-white shadow-[0_0_0_1px_rgba(0,0,0,0.6)]"
              style={{
                left: `${saturation * 100}%`,
                top: `${(1 - brightness) * 100}%`,
                background: value,
              }}
            />
          </div>

          <input
            type="range"
            aria-label="Hue"
            min={0}
            max={359}
            value={Math.round(hue)}
            onChange={(event) =>
              onChange(fromHsv(Number(event.target.value), saturation || 1, brightness || 1))
            }
            className="h-3 w-full cursor-pointer appearance-none rounded-pill outline-none
              [&::-webkit-slider-thumb]:h-4 [&::-webkit-slider-thumb]:w-4
              [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:rounded-pill
              [&::-webkit-slider-thumb]:border-2 [&::-webkit-slider-thumb]:border-white
              [&::-webkit-slider-thumb]:bg-transparent"
            style={{
              background:
                "linear-gradient(to right, #ff0000, #ffff00, #00ff00, #00ffff, #0000ff, #ff00ff, #ff0000)",
            }}
          />

          <div className="flex items-center gap-1.5">
            <input
              aria-label="Hex"
              value={typed ?? value.toUpperCase()}
              onChange={(event) => {
                const text = event.target.value.trim();
                setTyped(text);
                if (/^#?[0-9a-fA-F]{6}$/.test(text)) {
                  onChange(text.startsWith("#") ? text.toLowerCase() : `#${text.toLowerCase()}`);
                }
              }}
              // Half a colour is not one: what was typed goes, and the field
              // shows what is actually set.
              onBlur={() => setTyped(null)}
              className="w-[92px] rounded-inner border border-line bg-elevated px-2 py-1
                font-mono text-xs text-ink outline-none focus:border-line-strong"
            />
            {(["R", "G", "B"] as const).map((label, at) => (
              <label key={label} className="flex min-w-0 flex-1 items-center gap-1">
                <span className="text-[10px] text-ink-faint">{label}</span>
                <input
                  type="number"
                  min={0}
                  max={255}
                  value={[r, g, b][at]}
                  onChange={(event) => channel(at, Number(event.target.value))}
                  className="w-full min-w-0 rounded-inner border border-line bg-elevated px-1.5
                    py-1 text-xs text-ink outline-none tabular-nums focus:border-line-strong
                    [appearance:textfield]
                    [&::-webkit-inner-spin-button]:appearance-none
                    [&::-webkit-outer-spin-button]:appearance-none"
                />
              </label>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
