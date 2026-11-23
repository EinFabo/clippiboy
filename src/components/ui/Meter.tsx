import { type CSSProperties, type RefObject, useEffect, useRef } from "react";
import { cn } from "@/lib/cn";
import {
  decayLevel,
  type GainHold,
  type HeldPeak,
  holdGain,
  holdPeak,
  hotUntil,
  levelPct,
  meterGain,
  peakDbText,
  preGain,
  riseLevel,
} from "@/lib/meter";
import { type EngineState, useEngine } from "@/store";

/** Where a meter takes its level from: one source, or the clip's main track. */
export type LevelFeed = { source: string } | { mix: "l" | "r" | "peak" };

function reader(feed: LevelFeed): (s: EngineState) => number {
  if ("source" in feed) return (s) => s.levels[feed.source] ?? 0;
  const side = feed.mix;
  return side === "peak" ? (s) => Math.max(s.mix.l, s.mix.r) : (s) => s.mix[side];
}

const feedKey = (feed: LevelFeed) =>
  "source" in feed ? `source:${feed.source}` : `mix:${feed.mix}`;

type Entry = {
  read: (s: EngineState) => number;
  muted: boolean;
  /** Draw the signal before this gain (a volume track); the warning keeps the
   *  level after it, which is what reaches the clip. */
  gainDb: number | null;
  /** The gain before the last change, while packets may still carry it. */
  gainHold: GainHold | null;
  /** The level as drawn: rises at once, falls softly. */
  shown: number;
  /** The bar, moved by a transform. */
  fill?: HTMLElement;
  pct: number;
  /** Gets `data-hot` while the level is about to clip. */
  hotTarget?: HTMLElement | null;
  hotUntil: number;
  hot: boolean;
  /** A peak written out as dB. */
  text?: HTMLElement;
  held: HeldPeak;
  label: string;
};

// One store subscription and one frame loop serve every meter on the page. The
// core delivers levels 20 times a second; the bar is drawn with a transform, so
// nothing re-renders and nothing is laid out per frame.
const entries = new Set<Entry>();
let unsubscribe: (() => void) | null = null;
let frame = 0;
let last = 0;

function draw(e: Entry, now: number) {
  if (e.fill) {
    const pct = levelPct(e.shown);
    if (pct !== e.pct) {
      e.pct = pct;
      e.fill.style.transform = `scaleX(${pct / 100})`;
    }
  }
  if (e.hotTarget) {
    const hot = now < e.hotUntil;
    if (hot !== e.hot) {
      e.hot = hot;
      e.hotTarget.toggleAttribute("data-hot", hot);
    }
  }
  if (e.text) {
    const label = peakDbText(e.held.level);
    if (label !== e.label) {
      e.label = label;
      e.text.textContent = label;
    }
  }
}

function tick(now: number) {
  const dt = last ? now - last : 0;
  last = now;
  let moving = false;
  for (const e of entries) {
    if (e.shown > 0) e.shown = decayLevel(e.shown, dt);
    draw(e, now);
    // A warning still on has to be taken off once its hold is over.
    if (e.shown > 0 || e.hot) moving = true;
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

function feed(e: Entry, s: EngineState, now: number) {
  const level = e.muted ? 0 : e.read(s);
  e.shown = riseLevel(
    e.shown,
    e.gainDb === null ? level : preGain(level, meterGain(e.gainDb, e.gainHold, now)),
  );
  e.hotUntil = hotUntil(e.hotUntil, level, now);
  e.held = holdPeak(e.held, level, now);
}

function register(e: Entry) {
  entries.add(e);
  unsubscribe ??= useEngine.subscribe((s, prev) => {
    if (s.levels === prev.levels && s.mix === prev.mix) return;
    const now = performance.now();
    for (const m of entries) feed(m, s, now);
    wake();
  });
  feed(e, useEngine.getState(), performance.now());
  draw(e, performance.now());
  if (e.shown > 0) wake();
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

function newEntry(read: Entry["read"], muted: boolean): Entry {
  return {
    read,
    muted,
    gainDb: null,
    gainHold: null,
    shown: 0,
    pct: -1,
    hotUntil: 0,
    hot: false,
    held: { level: 0, at: 0 },
    label: "",
  };
}

/**
 * The live level inside a track: a bright bar with a soft glow that rises
 * instantly and falls softly. Fills its parent (the track, or the part of it
 * that is set) and takes no pointer, so a slider above it stays the control.
 * Reads its level from the store itself and draws outside React.
 *
 * With `gainDb` the bar shows the source's signal before that gain — how full
 * it is within the volume set — while `hot` (a `data-hot` attribute) still
 * follows the level after it, which is what clips in the clip.
 */
export function Meter({
  of,
  muted = false,
  gainDb,
  hot,
  className,
  style,
}: {
  of: LevelFeed;
  muted?: boolean;
  gainDb?: number;
  hot?: RefObject<HTMLElement | null>;
  className?: string;
  style?: CSSProperties;
}) {
  const fill = useRef<HTMLDivElement>(null);
  const entry = useRef<Entry | null>(null);
  const key = feedKey(of);

  useEffect(() => {
    const e = newEntry(reader(of), muted);
    e.gainDb = gainDb ?? null;
    e.fill = fill.current!;
    e.hotTarget = hot?.current;
    entry.current = e;
    register(e);
    return () => {
      unregister(e);
      e.hotTarget?.removeAttribute("data-hot");
    };
    // `muted` is handled below without tearing the entry down.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  useEffect(() => {
    const e = entry.current;
    if (!e) return;
    e.muted = muted;
    // Muted means nothing reaches the clip: no warning to hold on to.
    if (muted) e.hotUntil = 0;
  }, [muted]);

  // The gain the core applied — the stored one, not a knob still being dragged.
  // The levels catch up with a change a moment later; until then the old gain
  // is held as well (`meterGain`).
  useEffect(() => {
    const e = entry.current;
    if (!e) return;
    const next = gainDb ?? null;
    if (e.gainDb !== null && next !== null && next !== e.gainDb)
      e.gainHold = holdGain(e.gainHold, e.gainDb, performance.now());
    e.gainDb = next;
  }, [gainDb]);

  return (
    <div className={cn("pointer-events-none absolute inset-0", className)} style={style}>
      <div
        ref={fill}
        className="h-full w-full origin-left rounded-pill bg-gradient-to-r from-accent to-accent-bright
          shadow-[0_0_10px_color-mix(in_srgb,var(--color-accent-bright)_45%,transparent)]"
        // The glow is a plain shadow scaled with the bar: nothing is filtered
        // or laid out per frame.
        style={{ transform: "scaleX(0)", willChange: "transform" }}
      />
    </div>
  );
}

/** A level's peak in dB as text, held a second so it can be read. */
export function PeakText({ of, className }: { of: LevelFeed; className?: string }) {
  const text = useRef<HTMLSpanElement>(null);
  const key = feedKey(of);
  useEffect(() => {
    const e = newEntry(reader(of), false);
    e.text = text.current!;
    register(e);
    return () => unregister(e);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  return <span ref={text} className={className} />;
}
