import { useLayoutEffect, useRef, useState } from "react";
import { Card } from "@/components/ui/Card";
import { SLIDE, SlidingIndicator } from "@/components/ui/SlidingIndicator";
import { IconApp, IconGamepad } from "@/components/icons";
import { cn } from "@/lib/cn";
import { layoutMonitors, numberedMonitors, orderedWindows, placeGhost } from "@/lib/monitorMap";
import type { CaptureTarget, TargetKind, TargetRect } from "@/lib/types";

/** The saved source when it is not there: a screen unplugged, a window closed. */
export interface MissingSource {
  kind: TargetKind;
  /** Where the screen stood when it was picked, if that is known. */
  rect: TargetRect | null;
}

/** The key the list and the map share for the source nobody can pick. */
const MISSING = "missing";

const screenName = (t: CaptureTarget) => t.title.replace(/ — \d+×\d+$/, "");

/**
 * The Source card on the Video page: on the left every screen and window in the
 * settings' vertical list, on the right the screens as Windows arranges them
 * — or, with a window picked, that window.
 */
export function SourcePicker({
  targets,
  isPicked,
  missing,
  onPick,
}: {
  targets: CaptureTarget[];
  isPicked: (t: CaptureTarget) => boolean;
  missing: MissingSource | null;
  onPick: (t: CaptureTarget) => void;
}) {
  const monitors = numberedMonitors(targets);
  const windows = orderedWindows(targets);
  const picked = missing ? null : (targets.find(isPicked) ?? null);
  const activeKey = missing ? MISSING : (picked?.id ?? "");
  const number = (t: CaptureTarget) => monitors.indexOf(t) + 1;

  if (targets.length === 0 && !missing) {
    return (
      <Card className="px-5 py-4 text-xs text-ink-faint">
        No monitor or window found. Start the game and hit "Refresh".
      </Card>
    );
  }

  return (
    <Card className="grid min-h-[300px] grid-cols-[300px_minmax(0,1fr)] overflow-hidden">
      <div className="border-r border-line px-3.5 py-4">
        {/* The settings' vertical segmented control, with sources for tabs. */}
        <div
          role="group"
          aria-label="Source"
          className="relative flex max-h-[420px] flex-col gap-0.5 overflow-y-auto rounded-[20px]
            border border-line bg-elevated p-1"
        >
          <SlidingIndicator activeKey={activeKey} className="rounded-pill bg-white" />
          {monitors.map((t) => (
            <ListRow
              key={t.id}
              slide={t.id}
              on={activeKey === t.id}
              onClick={() => onPick(t)}
              badge={<NumberBadge n={number(t)} on={activeKey === t.id} />}
              name={screenName(t)}
              aside={`${t.width}×${t.height}`}
            />
          ))}
          {missing?.kind === "monitor" && (
            <ListRow
              slide={MISSING}
              on
              badge={<span className="size-[22px] shrink-0 rounded-[7px] border border-dashed border-ink-faint" />}
              name="Saved screen"
              aside={<span className="text-warn">Not connected</span>}
            />
          )}
          {(windows.length > 0 || missing?.kind === "window") && (
            <hr className="mx-2.5 my-1 border-0 border-t border-line" />
          )}
          {missing?.kind === "window" && (
            <ListRow
              slide={MISSING}
              on
              badge={<AppIcon icon={null} className="size-[22px] rounded-[7px]" />}
              name="Saved window"
              aside={<span className="text-warn">Closed</span>}
            />
          )}
          {windows.map((t) => (
            <ListRow
              key={t.id}
              slide={t.id}
              on={activeKey === t.id}
              onClick={() => onPick(t)}
              badge={<AppIcon icon={t.icon} className="size-[22px] rounded-[7px]" />}
              name={t.title}
              aside={
                t.isGame ? (
                  <span title="Detected game" className="flex items-center">
                    <IconGamepad className="size-3.5" />
                    <span className="sr-only">Detected game</span>
                  </span>
                ) : undefined
              }
            />
          ))}
        </div>
      </div>
      {picked?.kind === "window" || missing?.kind === "window" ? (
        <WindowView target={picked} />
      ) : (
        <MonitorMap
          monitors={monitors}
          picked={picked}
          missing={missing}
          number={number}
          onPick={onPick}
        />
      )}
    </Card>
  );
}

function ListRow({
  slide,
  on,
  onClick,
  badge,
  name,
  aside,
}: {
  slide: string;
  on: boolean;
  onClick?: () => void;
  badge: React.ReactNode;
  name: string;
  aside?: React.ReactNode;
}) {
  const className = cn(
    "relative flex h-9 w-full min-w-0 shrink-0 items-center gap-2.5 rounded-pill pr-3.5 pl-3 text-left text-[13px] font-medium",
    "transition-colors duration-150 ease-[var(--ease-out-soft)]",
    on ? "text-black" : "text-ink-muted hover:text-ink",
  );
  const content = (
    <>
      {badge}
      <span className="min-w-0 flex-1 truncate" title={name}>
        {name}
      </span>
      {aside && (
        <span className={cn("shrink-0 text-[11px]", on ? "text-[#6b6b75]" : "text-ink-faint")}>
          {aside}
        </span>
      )}
    </>
  );
  // The source that is not there cannot be picked — it already is.
  return onClick ? (
    <button type="button" {...{ [SLIDE]: slide }} aria-pressed={on} onClick={onClick} className={className}>
      {content}
    </button>
  ) : (
    <div {...{ [SLIDE]: slide }} className={className}>
      {content}
    </div>
  );
}

function NumberBadge({ n, on }: { n: number; on: boolean }) {
  return (
    <span
      className={cn(
        "grid size-[22px] shrink-0 place-items-center rounded-[7px] text-[11px] font-semibold",
        on ? "bg-accent text-white" : "bg-line text-ink",
      )}
    >
      {n}
    </span>
  );
}

/** The program's own icon, or a plain one when Windows would not give it. */
function AppIcon({ icon, className }: { icon: string | null; className?: string }) {
  return icon ? (
    <img src={icon} alt="" draggable={false} className={cn("shrink-0 object-contain", className)} />
  ) : (
    <span className={cn("grid shrink-0 place-items-center bg-line text-ink-muted", className)}>
      <IconApp className="size-[60%]" />
    </span>
  );
}

function CheckMark() {
  return (
    <span
      aria-hidden
      className="absolute top-2 right-2 grid size-[22px] place-items-center rounded-full bg-accent text-white"
    >
      <svg viewBox="0 0 12 12" className="size-3" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
        <path d="M2.5 6.5 5 9l4.5-5.5" />
      </svg>
    </span>
  );
}

/** The one line under the map or the window. */
function Meta({ children }: { children: React.ReactNode }) {
  return (
    <p className="absolute inset-x-6 bottom-[18px] truncate text-center text-[13px] text-ink-muted [&_b]:font-semibold [&_b]:text-ink">
      {children}
    </p>
  );
}

const hz = (t: CaptureTarget) => (t.refreshHz ? ` · ${t.refreshHz} Hz` : "");

function MonitorMap({
  monitors,
  picked,
  missing,
  number,
  onPick,
}: {
  monitors: CaptureTarget[];
  picked: CaptureTarget | null;
  missing: MissingSource | null;
  number: (t: CaptureTarget) => number;
  onPick: (t: CaptureTarget) => void;
}) {
  const area = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  useLayoutEffect(() => {
    const node = area.current;
    if (!node) return;
    const measure = () => setSize({ width: node.clientWidth, height: node.clientHeight });
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  // The unplugged screen counts for the frame too, so it keeps its place
  // beside the others instead of being drawn over them.
  const ghost =
    missing?.kind === "monitor" && missing.rect ? placeGhost(monitors, missing.rect) : null;
  const boxes = layoutMonitors([...monitors, ...(ghost ? [ghost] : [])], size);

  return (
    <div className="relative">
      <div ref={area} className="absolute inset-x-10 top-8 bottom-16">
        {monitors.map((t, at) => {
          const box = boxes[at];
          if (!box) return null;
          const on = picked?.id === t.id;
          const label = t.isPrimary ? `${number(t)} · Primary` : String(number(t));
          return (
            <button
              key={t.id}
              type="button"
              aria-pressed={on}
              aria-label={`Screen ${number(t)}, ${screenName(t)}, ${t.width}×${t.height}${t.isPrimary ? ", primary" : ""}`}
              onClick={() => onPick(t)}
              style={box}
              className={cn(
                "absolute overflow-hidden rounded-inner border bg-base",
                "transition-[border-color,box-shadow] duration-200 ease-[var(--ease-out-soft)]",
                on
                  ? "border-accent-bright shadow-[0_0_0_1px_var(--color-accent-bright)]"
                  : "border-line-strong hover:border-ink-faint",
              )}
            >
              {/* The slot for the screen's still image, under the number. */}
              <span className="absolute bottom-2 left-2 inline-flex h-[22px] items-center rounded-pill bg-black/60 px-[9px] text-xs font-semibold text-ink">
                {label}
              </span>
              {on && <CheckMark />}
            </button>
          );
        })}
        {/* After the live screens, and not in the way of a click on them. */}
        {ghost && boxes[monitors.length] && (
          <div
            role="img"
            aria-label="Saved screen, not connected"
            style={boxes[monitors.length]}
            className="pointer-events-none absolute rounded-inner border border-dashed border-ink-faint/70"
          >
            <span className="absolute bottom-2 left-2 inline-flex h-[22px] items-center rounded-pill bg-black/60 px-[9px] text-xs font-semibold text-warn">
              Not connected
            </span>
          </div>
        )}
      </div>
      {picked ? (
        <Meta>
          <b>{screenName(picked)}</b> · {picked.width}×{picked.height}
          {hz(picked)}
          {picked.isPrimary && " · Primary"}
        </Meta>
      ) : missing ? (
        <Meta>
          <b className="!text-warn">Not connected</b> · recording uses the primary screen
        </Meta>
      ) : null}
    </div>
  );
}

/** A picked window in place of the map: its program's icon and its name. */
function WindowView({ target }: { target: CaptureTarget | null }) {
  return (
    <div className="relative grid place-items-center pb-12">
      <div className="flex min-w-0 max-w-full flex-col items-center gap-4 px-10">
        {/* Where the window's still image will go. */}
        <AppIcon icon={target?.icon ?? null} className="size-16 rounded-[14px]" />
        <p className="max-w-full truncate text-[15px] font-semibold text-ink" title={target?.title}>
          {target ? target.title : "Saved window"}
        </p>
      </div>
      {target ? (
        <Meta>
          Window · {target.width}×{target.height}
          {hz(target)}
          {target.isGame && " · Detected game"}
        </Meta>
      ) : (
        <Meta>
          <b className="!text-warn">Closed</b> · recording uses the primary screen
        </Meta>
      )}
    </div>
  );
}
