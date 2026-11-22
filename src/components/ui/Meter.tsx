import { useEffect, useRef } from "react";
import { cn } from "@/lib/cn";
import { decayLevel, levelPct, riseLevel } from "@/lib/meter";
import { useEngine } from "@/store";

type Entry = {
  id: string;
  muted: boolean;
  fill: HTMLDivElement;
  shown: number;
  pct: number;
  hot: boolean;
};

// One store subscription and one frame loop serve every meter on the page. The
// core delivers levels 20 times a second; the bar is drawn with a transform, so
// nothing re-renders and nothing is laid out per frame.
const entries = new Set<Entry>();
let unsubscribe: (() => void) | null = null;
let frame = 0;
let last = 0;

function draw(e: Entry) {
  const pct = levelPct(e.shown);
  if (pct !== e.pct) {
    e.pct = pct;
    e.fill.style.transform = `scaleX(${pct / 100})`;
  }
  const hot = pct > 92;
  if (hot !== e.hot) {
    e.hot = hot;
    e.fill.style.background = hot
      ? "var(--color-live)"
      : "linear-gradient(90deg, var(--color-ok), var(--color-accent-bright))";
  }
}

function tick(now: number) {
  const dt = last ? now - last : 0;
  last = now;
  let moving = false;
  for (const e of entries) {
    if (e.shown > 0) e.shown = decayLevel(e.shown, dt);
    draw(e);
    if (e.shown > 0) moving = true;
  }
  if (moving) frame = requestAnimationFrame(tick);
  else {
    frame = 0;
    last = 0;
  }
}

function wake() {
  if (!frame) frame = requestAnimationFrame(tick);
}

function register(e: Entry) {
  entries.add(e);
  draw(e);
  unsubscribe ??= useEngine.subscribe((s, prev) => {
    if (s.levels === prev.levels) return;
    for (const m of entries) {
      m.shown = riseLevel(m.shown, m.muted ? 0 : (s.levels[m.id] ?? 0));
    }
    wake();
  });
  const level = useEngine.getState().levels[e.id] ?? 0;
  if (level > 0 && !e.muted) {
    e.shown = level;
    wake();
  }
}

function unregister(e: Entry) {
  entries.delete(e);
  if (entries.size) return;
  unsubscribe?.();
  unsubscribe = null;
  if (frame) cancelAnimationFrame(frame);
  frame = 0;
  last = 0;
}

/**
 * The level bar of one audio source: rises instantly, falls softly. Reads its
 * level from the store itself (by `source` id) and draws outside React.
 *
 * `variant="bar"` is the standalone 6 px pill. `variant="track"` has no height or
 * background of its own and fills its parent, for drawing behind another
 * control; `className` sizes either one.
 */
export function Meter({
  source,
  muted = false,
  variant = "bar",
  className,
}: {
  source: string;
  muted?: boolean;
  variant?: "bar" | "track";
  className?: string;
}) {
  const fill = useRef<HTMLDivElement>(null);
  const entry = useRef<Entry | null>(null);

  useEffect(() => {
    const e: Entry = {
      id: source,
      muted,
      fill: fill.current!,
      shown: 0,
      pct: -1,
      hot: false,
    };
    entry.current = e;
    register(e);
    return () => unregister(e);
    // `muted` is handled below without tearing the entry down.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [source]);

  useEffect(() => {
    if (entry.current) entry.current.muted = muted;
  }, [muted]);

  return (
    <div
      className={cn(
        "w-full overflow-hidden rounded-pill",
        variant === "bar" ? "h-1.5 bg-line" : "h-full",
        className,
      )}
    >
      <div
        ref={fill}
        className="h-full origin-left rounded-pill"
        style={{ transform: "scaleX(0)", willChange: "transform" }}
      />
    </div>
  );
}
