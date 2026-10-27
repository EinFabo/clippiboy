import { useEffect, useState } from "react";
import { createPortal } from "react-dom";

import { Button } from "@/components/ui/Button";
import { BusyRing } from "@/components/ui/BusyVeil";
import { api, events, inTauri } from "@/lib/ipc";
import { clipName } from "@/lib/format";
import { planExport } from "@/lib/exportPlan";
import { cn } from "@/lib/cn";
import type { Clip } from "@/lib/types";

const MB = 1024 * 1024;

/**
 * The limits worth having a button for. Discord's two are the reason this
 * feature exists at all; the third is for everywhere else that still has one.
 */
const PRESETS: Array<{ mb: number; label: string; hint: string }> = [
  { mb: 10, label: "10 MB", hint: "Discord" },
  { mb: 25, label: "25 MB", hint: "Discord Nitro" },
  { mb: 100, label: "100 MB", hint: "Most other places" },
];

/**
 * Ask for a size, then write a copy that fits it.
 *
 * Everything else in ClippiBoy aims at a quality and lets the size follow. This
 * is the one place that runs the other way, because "will it go through" is a
 * different question from "how good is it" — and a clip refused at the door is
 * no use however good it looks.
 */
export function ExportDialog({ clip, onClose }: { clip: Clip; onClose: () => void }) {
  const [targetMb, setTargetMb] = useState(25);
  const [custom, setCustom] = useState("");
  // "choosing" while the core's save dialog is open, "writing" from the
  // moment the core reports the copy under way.
  const [stage, setStage] = useState<"idle" | "choosing" | "writing">("idle");
  const busy = stage !== "idle";
  const [progress, setProgress] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const plan = planExport(clip, Math.floor(targetMb * MB));

  // Escape closes — but not mid-run, where it would leave a half-written file
  // behind with nothing saying so.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      if (event.key === "Escape" && !busy) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onClose]);

  useEffect(() => {
    if (!inTauri) return;
    let cancelled = false;
    let off: (() => void) | undefined;
    void events
      .onClipProgress((update) => {
        if (cancelled || update.clipId !== clip.id) return;
        setProgress(update.progress);
        setStage((now) => (now === "choosing" ? "writing" : now));
      })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else off = unlisten;
      });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [clip.id]);

  async function run() {
    // Not "<name> (<n> MB).mp4": that is the console's Discord copy, which goes
    // when the clip is edited or deleted (`manual_export_name` in the core).
    const suggested = `${clipName(clip).replace(/\.[^.]+$/, "")} - ${targetMb} MB.mp4`;
    setStage("choosing");
    setProgress(0);
    setError(null);
    try {
      // The core opens the save dialog itself and only takes the name as a
      // suggestion; `null` means it was cancelled, which leaves this open.
      const written = await api.exportClip(clip.id, Math.floor(targetMb * MB), suggested);
      if (written === null) setStage("idle");
      else onClose();
    } catch (failure) {
      // The core raises its own notice for a failure after it started, so this
      // is only a rejection that came before (serde, IPC).
      setError(`The export did not start: ${String(failure)}`);
      setStage("idle");
    }
  }

  return createPortal(
    <div
      className="fixed inset-0 z-50 grid place-items-center bg-black/60 backdrop-blur-sm"
      onClick={() => {
        if (!busy) onClose();
      }}
    >
      <div
        className="w-[420px] rounded-card border border-line bg-surface p-6 shadow-2xl"
        onClick={(event) => event.stopPropagation()}
      >
        {/* The dim layer covers the title bar, so the heading takes over
            moving the window. */}
        <h2 data-tauri-drag-region className="text-[16px] font-medium">
          Export a copy
        </h2>
        <p className="mt-1 text-xs text-ink-muted">
          The clip itself stays as it is — this writes a second file, small
          enough to send.
        </p>

        <div className="mt-5 space-y-2">
          {PRESETS.map((preset) => (
            <button
              key={preset.mb}
              disabled={busy}
              onClick={() => {
                setTargetMb(preset.mb);
                setCustom("");
              }}
              className={cn(
                "flex w-full items-center justify-between rounded-inner border px-4 py-3 text-left transition-colors",
                targetMb === preset.mb && custom === ""
                  ? "border-white/25 bg-elevated"
                  : "border-line hover:bg-elevated/60",
              )}
            >
              <span className="text-sm">{preset.label}</span>
              <span className="text-xs text-ink-faint">{preset.hint}</span>
            </button>
          ))}

          <div
            className={cn(
              "flex items-center justify-between gap-3 rounded-inner border px-4 py-3",
              custom !== "" ? "border-white/25 bg-elevated" : "border-line",
            )}
          >
            <label className="text-sm" htmlFor="export-size">
              Own size
            </label>
            <div className="flex items-center gap-2">
              <input
                id="export-size"
                type="number"
                step="any"
                min={1}
                max={4096}
                disabled={busy}
                value={custom}
                placeholder="—"
                onChange={(event) => {
                  const text = event.target.value;
                  setCustom(text);
                  const value = Number(text);
                  if (Number.isFinite(value) && value >= 1) setTargetMb(value);
                }}
                className="w-20 rounded-pill border border-line bg-transparent px-3 py-1
                  text-right font-mono text-sm outline-none focus:border-white/30"
              />
              <span className="text-xs text-ink-faint">MB</span>
            </div>
          </div>
        </div>

        <p className="mt-4 font-mono text-xs text-ink-muted">
          {plan.width}×{plan.height} · ~{(plan.videoKbps / 1000).toFixed(1)} Mbit/s ·{" "}
          {plan.seconds} s
        </p>
        {plan.height < clip.height && (
          <p className="mt-1 text-xs text-ink-faint">
            Scaled down from {clip.height}p — at this size a sharp smaller
            picture beats a blurred large one.
          </p>
        )}

        {/* Zero means the core has not reported anything yet, not that it is
            nought per cent through — so the mark walks instead of standing
            still at the bottom of its arc. */}
        {stage === "writing" && (
          <BusyRing
            className="mt-4 rounded-inner bg-elevated px-3 py-2.5"
            progress={progress > 0 ? progress : null}
            label="The copy is being written."
          />
        )}

        {error && (
          <p role="alert" className="mt-4 text-xs text-red-400">
            {error}
          </p>
        )}

        <div className="mt-6 flex justify-end gap-2">
          <Button variant="ghost" disabled={busy} onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            disabled={busy || !inTauri || targetMb < 1}
            onClick={() => void run()}
          >
            {stage === "writing" ? "Exporting…" : "Export"}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
