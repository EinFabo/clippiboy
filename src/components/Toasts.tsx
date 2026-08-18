import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "@/lib/ipc";
import { cn } from "@/lib/cn";

interface Notice {
  kind: "ok" | "error";
  message: string;
}

interface Entry extends Notice {
  id: number;
}

let nextId = 1;

export function Toasts() {
  const [entries, setEntries] = useState<Entry[]>([]);

  useEffect(() => {
    if (!inTauri) return;
    // StrictMode mountet den Effekt zweimal; `listen` löst aber erst nach dem
    // Cleanup auf. Ohne das Abbruch-Flag bleibt der erste Listener hängen und
    // jede Meldung erscheint doppelt.
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    listen<Notice>("notice", (event) => {
      const entry: Entry = { ...event.payload, id: nextId++ };
      setEntries((current) => [...current, entry]);
      setTimeout(
        () => setEntries((current) => current.filter((e) => e.id !== entry.id)),
        entry.kind === "error" ? 6000 : 3000,
      );
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  if (entries.length === 0) return null;

  return (
    <div className="pointer-events-none fixed bottom-6 left-1/2 z-50 flex -translate-x-1/2 flex-col items-center gap-2">
      {entries.map((entry) => (
        <div
          key={entry.id}
          className={cn(
            "rounded-pill border px-5 py-2.5 text-sm font-medium shadow-[0_8px_32px_rgba(0,0,0,0.5)]",
            "backdrop-blur-xl",
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
