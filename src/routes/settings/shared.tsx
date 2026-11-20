import { useId, useState } from "react";
import type { CaptureTarget, OverlayCorner } from "@/lib/types";
import { cn } from "@/lib/cn";
import { readOpen, writeOpen } from "@/lib/groupState";
import { Toggle } from "@/components/ui/Controls";

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

/** Under the usage stats switch, in the setup and in General. */
export const USAGE_HINT =
  "Counts, never content: version, how many clips, which features. No account, your IP is not stored. Details on clippiboy.com/privacy.html";

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
 * A collapsible group of rows inside a `Card`: a header row (title, status
 * summary, optional master switch, round chevron) and, when open, its rows below
 * with the card's dividers. Render it as a direct child of a `divide-y` card.
 * One level only; the open state is remembered per `storageKey`.
 */
export function SettingsGroup({
  title,
  summary,
  storageKey,
  defaultOpen = false,
  master,
  disabled = false,
  children,
}: {
  title: string;
  /** A status, shown muted on one line (not a hint). */
  summary?: React.ReactNode;
  /** Names the group in localStorage (`cb.settings.group.<key>`). */
  storageKey: string;
  defaultOpen?: boolean;
  /** Switch in the header; off dims the rows inside and makes them inert. */
  master?: { checked: boolean; onChange: (checked: boolean) => void; label?: string };
  /** Dims the rows inside like an off master does, for a switch outside the group; the group stays openable. */
  disabled?: boolean;
  children: React.ReactNode;
}) {
  const [open, setOpen] = useState(() => readOpen(storageKey, defaultOpen));
  const id = useId();
  const toggle = () => {
    const next = !open;
    setOpen(next);
    writeOpen(storageKey, next);
  };
  const off = disabled || (master !== undefined && !master.checked);
  return (
    <>
      {/* The whole header toggles; the switch keeps its click to itself. */}
      <div
        onClick={toggle}
        className="flex cursor-pointer items-center justify-between gap-6 px-5 py-3.5"
      >
        <p className="text-sm font-medium">{title}</p>
        <div className="flex min-w-0 items-center gap-3.5">
          {summary != null && (
            <span className="min-w-0 truncate text-[13px] text-ink-muted">{summary}</span>
          )}
          {master && (
            <span onClick={(e) => e.stopPropagation()}>
              <Toggle
                checked={master.checked}
                label={master.label ?? title}
                onChange={master.onChange}
              />
            </span>
          )}
          <button
            type="button"
            aria-expanded={open}
            aria-controls={id}
            aria-label={title}
            onClick={(e) => {
              e.stopPropagation();
              toggle();
            }}
            className="grid size-7 shrink-0 place-items-center rounded-pill border border-line
              bg-elevated text-ink-muted transition-colors hover:bg-hover hover:text-ink"
          >
            <svg
              viewBox="0 0 12 12"
              aria-hidden
              className={cn(
                "size-3 transition-transform duration-200 ease-[var(--ease-out-soft)] motion-reduce:transition-none",
                open && "rotate-180",
              )}
              fill="none"
              stroke="currentColor"
              strokeWidth="1.6"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M2.5 4.5 6 8l3.5-3.5" />
            </svg>
          </button>
        </div>
      </div>
      <div
        id={id}
        hidden={!open}
        inert={off ? true : undefined}
        className={cn("divide-y divide-line", off && "opacity-40")}
      >
        {children}
      </div>
    </>
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
  mark = "banner",
}: {
  /** `null` when the thing sits in no corner. */
  value: OverlayCorner | null;
  onChange: (corner: OverlayCorner) => void;
  disabled?: boolean;
  /** The banner's flat bar, or the cam's 16:9 box. */
  mark?: "banner" | "cam";
}) {
  return (
    <div
      className={cn(
        "grid grid-cols-2 gap-[3px] rounded-inner border border-line bg-elevated p-[3px]",
        disabled && "pointer-events-none opacity-40",
      )}
    >
      {corners.map(([corner, label, spot]) => (
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
              "absolute rounded-[2px]",
              mark === "cam" ? "h-2 w-3.5" : "h-1 w-3.5",
              "transition-colors duration-150 ease-[var(--ease-out-soft)]",
              spot,
              value === corner ? "bg-accent-bright" : "bg-ink-faint",
            )}
          />
        </button>
      ))}
    </div>
  );
}
