import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "@/lib/ipc";
import { cn } from "@/lib/cn";
import { useFlip } from "@/lib/useFlip";

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

export function Toasts() {
  const [entries, setEntries] = useState<Entry[]>([]);
  const stack = useRef<HTMLDivElement>(null);

  // Once one toast goes, the ones below it have to move up. Without this they
  // would jump the height of the gone one in a single frame.
  useFlip(
    stack,
    entries.map((e) => `${e.id}${e.leaving ? "-out" : ""}`).join(),
    200,
  );

  useEffect(() => {
    if (!inTauri) return;
    // StrictMode mounts the effect twice, but `listen` only resolves after the
    // cleanup. Without the cancelled flag the first listener stays around and
    // every message appears twice.
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    const timers: number[] = [];

    listen<Notice>("notice", (event) => {
      const entry: Entry = { ...event.payload, id: nextId++ };
      setEntries((current) => [...current, entry]);
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
          entry.kind === "error" ? 6000 : 3000,
        ),
      );
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });

    return () => {
      cancelled = true;
      unlisten?.();
      for (const timer of timers) clearTimeout(timer);
    };
  }, []);

  // No early return when the list is empty: it would take the last toast's node
  // away mid-fade, and the one message you were meant to read is exactly the
  // one that would vanish.
  return (
    <div
      ref={stack}
      className="pointer-events-none fixed bottom-6 left-1/2 z-50 flex -translate-x-1/2 flex-col items-center gap-2"
    >
      {entries.map((entry) => (
        <div
          key={entry.id}
          data-flip={entry.id}
          className={cn(
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
    </div>
  );
}
