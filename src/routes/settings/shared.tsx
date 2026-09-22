import type { CaptureTarget, OverlayCorner } from "@/lib/types";
import { cn } from "@/lib/cn";

/**
 * The four corners in reading order: value, what it is called, and where the
 * mark sits inside its cell of the pad below. One table drives all three.
 */
export const corners: [OverlayCorner, string, string][] = [
  ["topLeft", "top left", "top-1.5 left-1.5"],
  ["topRight", "top right", "top-1.5 right-1.5"],
  ["bottomLeft", "bottom left", "bottom-1.5 left-1.5"],
  ["bottomRight", "bottom right", "bottom-1.5 right-1.5"],
];

/**
 * The two entries in the screen dropdown that are not a device name. A monitor
 * id always reads `\\.\DISPLAYn`, so neither of them can be mistaken for one.
 */
export const FOLLOW = "follow";
export const UNKNOWN = "unknown";

/**
 * Welcher Eintrag im Bildschirm-Dropdown steht.
 *
 * Genauso gesucht, wie der Kern sucht: erst die Identität des Panels, dann der
 * Gerätename. `\\.\DISPLAYn` kann nach einem Neustart einen anderen Schirm
 * meinen, und dann zeigte das Dropdown etwas, das gar nicht in Gebrauch war.
 * `unplugged` heißt: gewählt ist etwas, das nicht angeschlossen ist — ohne den
 * Hinweis zeigte das Dropdown still seinen ersten Eintrag, während der Kern auf
 * den primären Schirm zurückfällt.
 */
export function screenChoice(
  monitors: CaptureTarget[],
  saved: { monitor: string | null; stableId: string | null; follow: boolean },
): { value: string | null; unplugged: boolean } {
  if (saved.follow) return { value: FOLLOW, unplugged: false };
  const found =
    monitors.find((m) => saved.stableId !== null && m.stableId === saved.stableId) ??
    monitors.find((m) => saved.monitor === m.id) ??
    (saved.monitor === null && saved.stableId === null
      ? monitors.find((m) => m.isPrimary)
      : undefined) ??
    null;
  return {
    value: found?.id ?? null,
    unplugged: found === null && monitors.length > 0,
  };
}

export function Row({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-6 p-5">
      <div className="min-w-0">
        <p className="text-sm font-medium">{label}</p>
        {hint && <p className="mt-1 text-xs text-ink-muted">{hint}</p>}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

/**
 * The four corners as a 2 × 2 pad: every cell is a screen, and the little bar in
 * it sits where the banner would. Four words in a row said where it goes — this
 * shows it, in less room than the words took.
 */
export function CornerPad({
  value,
  onChange,
  disabled,
}: {
  value: OverlayCorner;
  onChange: (corner: OverlayCorner) => void;
  disabled?: boolean;
}) {
  return (
    <div
      className={cn(
        "grid grid-cols-2 gap-[3px] rounded-inner border border-line bg-elevated p-[3px]",
        disabled && "pointer-events-none opacity-40",
      )}
    >
      {corners.map(([corner, label, mark]) => (
        <button
          key={corner}
          type="button"
          aria-label={label}
          aria-pressed={value === corner}
          onClick={() => onChange(corner)}
          className={cn(
            "relative h-7 w-10 rounded-[9px]",
            "transition-colors duration-150 ease-[var(--ease-out-soft)]",
            value === corner ? "bg-accent/25" : "bg-base hover:bg-hover",
          )}
        >
          {/* Roughly the banner's proportions — 416 × 128 in `overlay.rs`. */}
          <span
            className={cn(
              "absolute h-1 w-3.5 rounded-[2px]",
              "transition-colors duration-150 ease-[var(--ease-out-soft)]",
              mark,
              value === corner ? "bg-accent-bright" : "bg-ink-faint",
            )}
          />
        </button>
      ))}
    </div>
  );
}
