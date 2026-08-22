import { useEffect, useRef } from "react";
import { cn } from "@/lib/cn";
import {
  IconAudio,
  IconClips,
  IconHome,
  IconRecord,
  IconSettings,
} from "./icons";
import { SLIDE, SlidingIndicator } from "./ui/SlidingIndicator";
import { LiveDot } from "./ui/LiveDot";
import { animate, EASE_SPRING } from "@/lib/motion";
import { useEngine } from "@/store";

export type Route = "dashboard" | "clips" | "audio" | "recording" | "settings";

const items: Array<{ id: Route; label: string; icon: typeof IconHome }> = [
  { id: "dashboard", label: "Overview", icon: IconHome },
  { id: "clips", label: "Clips", icon: IconClips },
  { id: "audio", label: "Audio", icon: IconAudio },
  { id: "recording", label: "Recording", icon: IconRecord },
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

        <BufferBadge active={bufferActive} />
      </div>
    </nav>
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
