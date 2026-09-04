import {
  forwardRef,
  useEffect,
  useId,
  useImperativeHandle,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import { freezeFrame } from "@/lib/dom";
import { prefersReducedMotion } from "@/lib/motion";
import { cn } from "@/lib/cn";

/**
 * What the app shows while it is writing a file.
 *
 * Two marks, because there are two surfaces and they are not the same problem.
 * Over the player there is a picture to work on, so [SaveVeil] freezes it and
 * walks a head across it — the file really is being recomputed, and that is
 * what it looks like. The export dialog is 420 px of nothing, so [BusyRing]
 * fills the logo's own C instead.
 *
 * Both move on opacity, transform and stroke-dashoffset only, which is the rule
 * styles/motion.css sets itself at the top: nothing here may cost a layout pass
 * while a game is running next door.
 */

/** Has to match the length of `cb-veil-out` in styles/motion.css. */
const LEAVE_MS = 200;

/**
 * A percentage as the ring and the head want it, or `null` while there is
 * nothing to report — a clip that is only rewritten is finished before the core
 * sends its first number.
 */
type Progress = number | null;

const clamp = (value: number) => Math.min(1, Math.max(0, value));

/* -------------------------------------------------------------------------
   The player
   ------------------------------------------------------------------------- */

export interface SaveVeilHandle {
  /**
   * Take the picture. Has to be called *before* the video lets go of the file —
   * an element whose `src` is already gone has nothing left to draw.
   */
  freeze: (video: HTMLVideoElement | null) => void;
}

interface SaveVeilProps {
  /** Is a file being written right now? */
  active: boolean;
  progress: Progress;
  /** One word. The sentence belongs to the editor pane beside the picture. */
  label: string;
}

export const SaveVeil = forwardRef<SaveVeilHandle, SaveVeilProps>(
  function SaveVeil({ active, progress, label }, ref) {
    // The still ahead of the head, in grey, and the one behind it in colour.
    // Two canvases rather than one and a filter: a filter cannot be applied to
    // half an element, and the split is the whole point.
    const todo = useRef<HTMLCanvasElement>(null);
    const done = useRef<HTMLCanvasElement>(null);
    const [held, setHeld] = useState(false);
    /** Has this run reported a number at all? */
    const reported = useRef(false);
    if (progress !== null) reported.current = true;

    useImperativeHandle(ref, () => ({
      freeze(video) {
        freezeFrame(video, [todo.current, done.current]);
      },
    }));

    // Stays a moment after it is over, so the exit can be seen. The canvases
    // themselves are never unmounted — `freeze` has to find them in the
    // document before React has heard that anything is being saved.
    useEffect(() => {
      if (active) {
        setHeld(true);
        return;
      }
      if (!held) return;
      const timer = window.setTimeout(() => {
        setHeld(false);
        reported.current = false;
      }, LEAVE_MS);
      return () => clearTimeout(timer);
    }, [active, held]);

    // The core stops reporting the moment it is done, but the veil stays up a
    // little longer while the fresh element loads. Dropping back to the walking
    // head there would fling it back to the left in the last second of every
    // save — so a run that has reported once finishes its journey instead.
    // ffmpeg stops short at 99 %; this is what carries the head home.
    const shown = progress ?? (active && reported.current ? 1 : null);

    // Until the first number arrives the head walks by itself. With movement
    // turned down it does not walk at all and the still simply stays grey.
    const idle = shown === null;
    const walks = idle && !prefersReducedMotion();

    return (
      <div
        aria-hidden={!active}
        style={idle ? undefined : ({ "--cb-head": `${clamp(shown) * 100}%` } as CSSProperties)}
        className={cn(
          "pointer-events-none absolute inset-0",
          !idle && "cb-sweep",
          walks && "cb-head-idle",
          !held && "hidden",
          held && (active ? "cb-veil-in" : "cb-veil-out"),
        )}
      >
        <canvas ref={todo} className="cb-sweep-todo absolute inset-0 h-full w-full object-contain" />
        <canvas
          ref={done}
          className="cb-sweep-done absolute inset-0 h-full w-full object-contain"
        />

        {/* The head. A full-width wrapper that the transform slides along, with
            the line and its glow sitting at the wrapper's left edge — that way
            nothing but a transform changes while it travels. Hidden only when
            there is neither a number to place it by nor the movement to carry
            it. */}
        {!(idle && !walks) && (
          <span className="cb-head absolute inset-y-0 left-0 w-full">
            <span className="cb-head-glow absolute inset-y-0 left-0 w-14 -translate-x-1/2" />
            <span className="cb-head-line absolute inset-y-0 left-0 w-0.5 -translate-x-1/2" />
          </span>
        )}

        <div
          className="absolute top-1/2 left-1/2 flex -translate-x-1/2 -translate-y-1/2 items-center
            gap-2.5 rounded-pill border border-white/10 bg-black/75 py-2 pr-3.5 pl-2.5"
        >
          <Mark className="h-7 w-7" />
          <span className="font-mono text-[13px] tabular-nums">
            {idle ? label : `${Math.round(clamp(shown) * 100)} %`}
          </span>
        </div>
      </div>
    );
  },
);

/* -------------------------------------------------------------------------
   The export dialog
   ------------------------------------------------------------------------- */

/**
 * The logo, filling up. The bright arc runs the C's own path — in at the lower
 * end, out at the upper one — so the opening the play triangle points through
 * is never painted over. The mark stays the mark; it does not close into a ring.
 */
export function BusyRing({
  progress,
  label,
  className,
}: {
  progress: Progress;
  label: string;
  className?: string;
}) {
  const idle = progress === null;
  return (
    <div className={cn("flex items-center gap-3", className)}>
      <Mark
        className="h-11 w-11 shrink-0"
        arc={
          <path
            d={ARC}
            pathLength={100}
            fill="none"
            strokeWidth="28"
            strokeLinecap="round"
            className={cn("cb-ring-arc", idle && "cb-ring-idle")}
            style={idle ? undefined : { strokeDashoffset: 100 - clamp(progress) * 100 }}
          />
        }
      />
      <span className="text-xs text-ink-muted">{label}</span>
      {!idle && (
        <span className="ml-auto font-mono text-xs text-ink tabular-nums">
          {Math.round(clamp(progress) * 100)} %
        </span>
      )}
    </div>
  );
}

/* -------------------------------------------------------------------------
   The mark
   ------------------------------------------------------------------------- */

/** The C, as it is drawn in assets/logo.svg. */
const C = "M177.03 86.86 A64 64 0 1 0 177.03 169.14";
/** The same arc backwards, so a dash along it starts at the lower end. */
const ARC = "M177.03 169.14 A64 64 0 1 1 177.03 86.86";
const PLAY = "M111.6 102.1 L143.2 128 L111.6 153.9 Z";

/**
 * ClippiBoy's logo, inline so parts of it can be moved.
 *
 * The gradient ids are made unique per instance: two of these on screen at once
 * with the same ids would have the second one quietly borrow the first one's
 * definitions.
 */
function Mark({ className, arc }: { className?: string; arc?: React.ReactNode }) {
  const unique = useId().replace(/:/g, "");
  const disc = `${unique}-disc`;
  const rim = `${unique}-rim`;
  const mark = `${unique}-mark`;
  const play = `${unique}-play`;

  return (
    <svg viewBox="0 0 256 256" className={className} aria-hidden>
      <defs>
        <radialGradient id={disc} cx="34%" cy="24%" r="92%">
          <stop offset="0" stopColor="#1d1533" />
          <stop offset=".55" stopColor="#110c1f" />
          <stop offset="1" stopColor="#08060f" />
        </radialGradient>
        <linearGradient id={rim} x1="0" y1="0" x2=".45" y2="1">
          <stop offset="0" stopColor="#7c4fd8" />
          <stop offset=".5" stopColor="#452a7f" />
          <stop offset="1" stopColor="#2a1a4d" />
        </linearGradient>
        <linearGradient id={mark} x1=".08" y1=".05" x2=".92" y2=".95">
          <stop offset="0" stopColor="#b492ff" />
          <stop offset=".45" stopColor="#8b5cf6" />
          <stop offset="1" stopColor="#6d28d9" />
        </linearGradient>
        <linearGradient id={play} x1=".1" y1="0" x2=".9" y2="1">
          <stop offset="0" stopColor="#a78bfa" />
          <stop offset="1" stopColor="#7c3aed" />
        </linearGradient>
      </defs>

      <circle cx="128" cy="128" r="125" fill={`url(#${disc})`} />
      <circle cx="128" cy="128" r="123.5" fill="none" stroke={`url(#${rim})`} strokeWidth="3" />
      {/* With an arc on top the C steps back and becomes the track it runs on. */}
      <path
        d={C}
        fill="none"
        stroke={`url(#${mark})`}
        strokeWidth="28"
        strokeLinecap="round"
        opacity={arc ? 0.26 : 1}
      />
      {arc && <g stroke={`url(#${mark})`}>{arc}</g>}
      <path
        d={PLAY}
        fill={`url(#${play})`}
        stroke={`url(#${play})`}
        strokeWidth="9"
        strokeLinejoin="round"
      />
    </svg>
  );
}
