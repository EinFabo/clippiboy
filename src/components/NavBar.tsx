import { useEffect, useRef } from "react";
import { cn } from "@/lib/cn";
import {
  IconAudio,
  IconClips,
  IconHome,
  IconMonitor,
  IconRecord,
  IconSettings,
} from "./icons";
import { formatDuration } from "@/lib/format";
import { SLIDE, SlidingIndicator } from "./ui/SlidingIndicator";
import { LiveDot } from "./ui/LiveDot";
import { animate, EASE_SPRING } from "@/lib/motion";
import { useEngine } from "@/store";

export type Route = "dashboard" | "clips" | "audio" | "recording" | "settings";

const items: Array<{ id: Route; label: string; icon: typeof IconHome }> = [
  { id: "dashboard", label: "Overview", icon: IconHome },
  { id: "clips", label: "Clips", icon: IconClips },
  { id: "audio", label: "Audio", icon: IconAudio },
  // "Video", not "Recording": since recordings exist, the word means the
  // thing started by hand — this page is about how the picture is captured.
  { id: "recording", label: "Video", icon: IconMonitor },
  { id: "settings", label: "Settings", icon: IconSettings },
];

export function NavBar({
  route,
  onNavigate,
}: {
  route: Route;
  onNavigate: (r: Route) => void;
}) {
  const bufferActive = useEngine((s) => s.bufferActive);
  const recording = useEngine((s) => s.recording);
  const recordingSeconds = useEngine((s) => s.recordingSeconds);
  const recordingProgress = useEngine((s) => s.recordingProgress);

  return (
    <nav className="absolute inset-x-0 top-12 z-40 flex justify-center px-8">
      <div
        className="relative flex items-center gap-1 rounded-pill border border-white/10 p-1.5 pr-2 shadow-[0_8px_32px_rgba(0,0,0,0.4)]"
        style={{ background: "var(--glass)", backdropFilter: "blur(20px)" }}
      >
        {/* One marker for the whole bar instead of a background per button: it
            travels, and that is what makes the switch read as a move rather
            than as two separate things happening. */}
        <SlidingIndicator activeKey={route} className="rounded-pill bg-white/12" />

        {items.map(({ id, label, icon: Icon }) => (
          <button
            key={id}
            {...{ [SLIDE]: id }}
            onClick={() => onNavigate(id)}
            className={cn(
              // Positioned, so it stands above the marker instead of under it.
              "relative inline-flex h-9 items-center gap-2 rounded-pill px-4 text-[13px] font-medium",
              "transition-colors duration-150 ease-[var(--ease-out-soft)]",
              route === id ? "text-ink" : "text-ink-muted hover:text-ink",
            )}
          >
            <Icon className="h-4 w-4" />
            {label}
          </button>
        ))}

        <span className="mx-1 h-5 w-px bg-white/10" />

        {recording && <RecordingBadge seconds={recordingSeconds} />}
        {!recording && recordingProgress !== null && (
          <SavingBadge share={recordingProgress} />
        )}
        <BufferBadge active={bufferActive} />
      </div>
    </nav>
  );
}

/** Only there while a recording runs: the red light and how long it is. */
function RecordingBadge({ seconds }: { seconds: number }) {
  return (
    <span
      className="relative inline-flex h-9 items-center gap-2 rounded-pill px-4 text-[13px] font-medium text-live tabular-nums"
      title="Recording"
    >
      <IconRecord className="h-4 w-4" />
      {formatDuration(seconds * 1000)}
    </span>
  );
}

/** Writing a stopped recording out — visible on every page, since it can
    take a while and the stop may have come from a hotkey. */
function SavingBadge({ share }: { share: number }) {
  return (
    <span
      className="relative inline-flex h-9 items-center gap-2 rounded-pill px-4 text-[13px] font-medium text-ink-muted tabular-nums"
      title="Saving the recording"
    >
      <IconRecord className="h-4 w-4" />
      Saving {Math.round(share * 100)} %
    </span>
  );
}

/**
 * Whether the buffer is running — and, for one moment, that it just changed.
 *
 * The flare is the same gesture as `cb-glow` in the overlay: brief, then gone.
 * It does not stay lit, because the dot already says what the state is; the
 * light is only there to say that something happened just now.
 */
function BufferBadge({ active }: { active: boolean }) {
  const glow = useRef<HTMLSpanElement>(null);
  const dot = useRef<HTMLSpanElement>(null);
  const was = useRef(active);

  useEffect(() => {
    if (was.current === active) return;
    was.current = active;

    animate(
      glow.current,
      active
        ? [{ opacity: 0 }, { opacity: 0.9, offset: 0.7 }, { opacity: 0 }]
        : // Mirrored: the flare comes early on the way out, the way it does in
          // `cb-unglow`.
          [{ opacity: 0 }, { opacity: 0.9, offset: 0.3 }, { opacity: 0 }],
      { duration: 400, easing: "ease-out" },
    );

    if (active) {
      animate(
        dot.current,
        [
          { transform: "scale(0.6)" },
          { transform: "scale(1.15)", offset: 0.55 },
          { transform: "scale(1)" },
        ],
        { duration: 400, easing: EASE_SPRING },
      );
    }
  }, [active]);

  return (
    <span
      className={cn(
        "relative inline-flex h-9 items-center gap-2 rounded-pill px-4 text-[13px] font-medium",
        "transition-colors duration-200 ease-[var(--ease-out-soft)]",
        active ? "text-ink" : "text-ink-faint",
      )}
      title={active ? "Replay buffer running" : "Replay buffer off"}
    >
      <span
        ref={glow}
        aria-hidden
        style={{ opacity: 0 }}
        className="pointer-events-none absolute inset-0 rounded-pill bg-live/25 blur-[6px]"
      />
      <span ref={dot} className="relative grid place-items-center">
        {active ? (
          <LiveDot />
        ) : (
          <span className="h-2 w-2 rounded-pill bg-line-strong" />
        )}
      </span>
      <span className="relative">{active ? "Buffer on" : "Buffer off"}</span>
    </span>
  );
}
