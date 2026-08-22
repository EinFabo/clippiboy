import { type CSSProperties, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { fileUrl, inTauri } from "@/lib/ipc";
import type { OverlayBanner } from "@/lib/types";
import { cn } from "@/lib/cn";

/** How long the fade-out animation runs — has to match `cb-card-out`. */
const LEAVE_MS = 240;

interface Shown extends OverlayBanner {
  /** Increments with every banner; forces a remount so the CSS animations run
   *  cleanly from the start. */
  seq: number;
}

/**
 * The banner shown over the game.
 *
 * The window itself is shown, positioned and hidden again by Rust; this is only
 * about content and animation. It is click-through, so there are deliberately no
 * buttons in it.
 */
export function Overlay() {
  const [banner, setBanner] = useState<Shown | null>(null);
  const [leaving, setLeaving] = useState(false);
  const timers = useRef<number[]>([]);
  const seq = useRef(0);

  useEffect(() => {
    if (!inTauri) {
      // In the browser (`npm run dev`) show a sample banner so the look can be
      // checked without a Windows build.
      setBanner({
        kind: "clip",
        title: "Counter-Strike 2",
        detail: "Clip saved · 32 s",
        thumbPath: null,
        durationMs: 3500,
        seq: 0,
      });
      return;
    }

    // See Toasts.tsx: without the cancelled flag the StrictMode double-mount
    // leaves one listener too many behind.
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    listen<OverlayBanner>("overlay-banner", (event) => {
      timers.current.forEach(clearTimeout);
      timers.current = [];

      seq.current += 1;
      setLeaving(false);
      setBanner({ ...event.payload, seq: seq.current });

      timers.current.push(
        window.setTimeout(() => setLeaving(true), event.payload.durationMs),
        window.setTimeout(
          () => setBanner(null),
          event.payload.durationMs + LEAVE_MS,
        ),
      );
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });

    const running = timers.current;
    return () => {
      cancelled = true;
      running.forEach(clearTimeout);
      unlisten?.();
    };
  }, []);

  if (!banner) return null;

  const accent = {
    clip: "text-accent-bright",
    buffer: "text-ok",
    bufferOff: "text-ok",
    error: "text-live",
    info: "text-ink-muted",
    // The same colour as a clip: both say "kept". What tells them apart is the
    // sign in the picture frame, and that is enough at a glance.
    screenshot: "text-accent-bright",
  }[banner.kind];

  const stroke = {
    clip: "var(--color-accent-bright)",
    buffer: "var(--color-ok)",
    bufferOff: "var(--color-ok)",
    error: "var(--color-live)",
    info: "var(--color-line-strong)",
    screenshot: "var(--color-accent-bright)",
  }[banner.kind];

  // "Buffer off" plays the line backwards and greys it out along the way — the
  // counterpart to switching on.
  const rewind = banner.kind === "bufferOff";

  const thumb = fileUrl(banner.thumbPath);

  return (
    <div className="flex h-full w-full items-center justify-center p-3">
      <div
        key={banner.seq}
        data-leaving={leaving}
        className={cn(
          "cb-card relative flex w-full items-center gap-3.5 rounded-card p-3",
          "shadow-[0_16px_48px_rgba(0,0,0,0.55)] backdrop-blur-2xl",
          banner.kind === "error" ? "bg-live/15" : "bg-[var(--glass)]",
        )}
      >
        {/* The line runs once around the card and then fades out. */}
        <svg
          className={cn(
            "cb-outline pointer-events-none absolute inset-0 h-full w-full overflow-visible",
            rewind && "cb-outline-rewind",
          )}
          style={{ "--cb-stroke": stroke } as CSSProperties}
          aria-hidden
        >
          {/* pathLength normalizes the perimeter to 100 — otherwise the dash
              length would depend on the card's actual size. */}
          <rect className="cb-halo" pathLength={100} />
          <rect className="cb-line" pathLength={100} />
        </svg>

        <div className="relative aspect-video h-[72px] shrink-0 overflow-hidden rounded-inner bg-gradient-to-br from-accent-deep/50 to-black">
          {thumb ? (
            <img src={thumb} alt="" className="h-full w-full object-cover" />
          ) : (
            <div className={cn("grid h-full w-full place-items-center", accent)}>
              <Mark kind={banner.kind} />
            </div>
          )}
        </div>

        <div className="relative min-w-0 flex-1">
          <p className={cn("text-[11px] font-semibold tracking-wide uppercase", accent)}>
            ClippiBoy
          </p>
          <p className="mt-0.5 truncate text-[15px] font-semibold text-ink">
            {banner.title}
          </p>
          {banner.detail && (
            <p className="mt-0.5 truncate text-xs text-ink-muted">
              {banner.detail}
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

function Mark({ kind }: { kind: OverlayBanner["kind"] }) {
  if (kind === "error") {
    return (
      <svg viewBox="0 0 24 24" className="h-7 w-7" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
        <circle cx="12" cy="12" r="9" />
        <path d="M12 7.5v5.5M12 16.3v.2" />
      </svg>
    );
  }
  if (kind === "screenshot") {
    return (
      <svg viewBox="0 0 24 24" className="h-7 w-7" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinejoin="round">
        <path d="M3 8.5A1.5 1.5 0 0 1 4.5 7h2.2l1.3-2h7.9l1.3 2h2.3A1.5 1.5 0 0 1 21 8.5v9a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5v-9Z" />
        <circle cx="12" cy="13" r="3.4" />
      </svg>
    );
  }
  if (kind === "buffer" || kind === "bufferOff") {
    return (
      <svg viewBox="0 0 24 24" className="h-7 w-7" fill="none" stroke="currentColor" strokeWidth="1.8">
        <circle cx="12" cy="12" r="8.5" />
        <circle cx="12" cy="12" r="3.5" fill="currentColor" stroke="none" />
      </svg>
    );
  }
  return (
    <svg viewBox="0 0 24 24" className="h-7 w-7" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinejoin="round">
      <rect x="3" y="5" width="18" height="14" rx="3" />
      <path d="M10 9.5v5l4.5-2.5L10 9.5Z" />
    </svg>
  );
}
