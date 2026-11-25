import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useFullscreenHost } from "@/lib/useFullscreenHost";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "@/lib/ipc";
import { cn } from "@/lib/cn";
import { useFlip } from "@/lib/useFlip";
import { onToast } from "@/lib/toastBus";
import { isDuplicateToast } from "@/lib/toastDedupe";
import { signInCardSays } from "@/lib/friends";

interface Notice {
  kind: "ok" | "error";
  message: string;
}

interface Entry extends Notice {
  id: number;
  /** Fading out — the entry is only still here so it can be seen going. */
  leaving?: boolean;
}

let nextId = 1;

/** Has to match the length of `cb-toast-out` in styles/motion.css. */
const LEAVE_MS = 200;
const MAX_SHOWN = 3;

export function Toasts() {
  const host = useFullscreenHost();
  const [entries, setEntries] = useState<Entry[]>([]);
  const stack = useRef<HTMLDivElement>(null);
  // Bottom offset that clears the transfer cards (same corner), or null when
  // there are none and the usual corner spot is right.
  const [lift, setLift] = useState<number | null>(null);

  // Once one toast goes, the ones below it have to move up. Without this they
  // would jump the height of the gone one in a single frame.
  useFlip(
    stack,
    entries.map((e) => `${e.id}${e.leaving ? "-out" : ""}`).join(),
    200,
  );

  // The transfer cards come and go and move up over a full view; looked at
  // while a toast is up, which is the only time it matters.
  useLayoutEffect(() => {
    const measure = () => {
      const cards = document.querySelector("[data-transfers]");
      const next = cards ? Math.round(window.innerHeight - cards.getBoundingClientRect().top + 8) : null;
      setLift((now) => (now === next ? now : next));
    };
    measure();
    if (entries.length === 0) return;
    const timer = window.setInterval(measure, 300);
    return () => window.clearInterval(timer);
  }, [entries.length]);

  useEffect(() => {
    const timers: number[] = [];
    // The core's `notice` and the caller's own toast can say the same thing.
    let last: { kind: string; message: string; at: number } | null = null;
    const push = (notice: Notice) => {
      const now = Date.now();
      if (isDuplicateToast(last, notice, now)) return;
      last = { kind: notice.kind, message: notice.message, at: now };
      const entry: Entry = { ...notice, id: nextId++ };
      // At most three at once: the oldest drop off first.
      setEntries((current) => [...current.filter((e) => !e.leaving), entry].slice(-MAX_SHOWN));
      // Errors stay until clicked away.
      if (entry.kind === "error") return;
      // Two steps: first the entry is marked as going and plays its exit, then
      // it actually falls out of the list.
      timers.push(
        window.setTimeout(
          () => {
            setEntries((current) =>
              current.map((e) => (e.id === entry.id ? { ...e, leaving: true } : e)),
            );
            timers.push(
              window.setTimeout(
                () => setEntries((current) => current.filter((e) => e.id !== entry.id)),
                LEAVE_MS,
              ),
            );
          },
          3000,
        ),
      );
    };

    // Toasts raised by this window itself, beside the core's `notice` events.
    const offBus = onToast(push);

    // StrictMode mounts the effect twice, but `listen` only resolves after the
    // cleanup. Without the cancelled flag the first listener stays around and
    // every message appears twice.
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    if (inTauri) {
      listen<Notice>("notice", (event) => {
        // The Friends page's sign-in card says why a sign-in failed itself.
        if (signInCardSays(event.payload)) return;
        push(event.payload);
      }).then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      });
    }

    return () => {
      cancelled = true;
      offBus();
      unlisten?.();
      for (const timer of timers) clearTimeout(timer);
    };
  }, []);

  // No early return when the list is empty: it would take the last toast's node
  // away mid-fade, and the one message you were meant to read is exactly the
  // one that would vanish. In fullscreen the stack moves into the player's
  // frame, the only thing drawn then.
  return createPortal(
    <div
      ref={stack}
      className="pointer-events-none fixed right-6 bottom-6 z-[65] flex flex-col items-end gap-2 [body:has([data-fullview])_&]:bottom-36"
      style={lift === null ? undefined : { bottom: lift }}
    >
      {entries.map((entry) => (
        <div
          key={entry.id}
          data-flip={entry.id}
          onClick={() => setEntries((current) => current.filter((e) => e.id !== entry.id))}
          className={cn(
            entry.kind === "error" && "pointer-events-auto cursor-pointer",
            "rounded-pill border px-5 py-2.5 text-sm font-medium shadow-[0_8px_32px_rgba(0,0,0,0.5)]",
            "backdrop-blur-xl",
            entry.leaving ? "cb-toast-out" : "cb-toast-in",
            entry.kind === "error"
              ? "border-live/40 bg-live/20 text-white"
              : "border-white/12 bg-elevated/90 text-ink",
          )}
        >
          {entry.message}
        </div>
      ))}
    </div>,
    host,
  );
}
