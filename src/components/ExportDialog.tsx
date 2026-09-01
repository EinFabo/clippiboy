import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";

import { Button } from "@/components/ui/Button";
import { api, events, inTauri } from "@/lib/ipc";
import { clipName } from "@/lib/format";
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

/** The steps the export scales down to, mirroring `export::HEIGHTS`. */
const HEIGHTS = [1080, 720, 480];

/**
 * What the export will do, worked out the same way `export::recipe` does it.
 *
 * Deliberately duplicated rather than asked of the core: this runs on every
 * keystroke in the size field, and a round trip per character to tell somebody
 * what they are about to get would be a poor trade. The two must be kept in
 * step — the tests in `export.rs` are the ones that matter.
 */
function preview(clip: Clip, targetMb: number) {
  const seconds = Math.max(1, Math.round(clip.durationMs / 1000));
  const usableBits = targetMb * MB * 8 * 0.97;
  const videoKbps = Math.max(
    200,
    Math.floor((usableBits - 128 * 1000 * seconds) / seconds / 1000),
  );
  let height = clip.height;
  for (const step of HEIGHTS) {
    if (step > clip.height) continue;
    height = step;
    // `bitrate_for` at 30 fps, halved — see `export::STARVED`.
    const width = Math.max(2, Math.round((clip.width * step) / clip.height)) & ~1;
    const wants = Math.round((width * step * 30 * 0.15) / 1000 / 1000) * 1000;
    if (videoKbps >= wants / 2) break;
  }
  const width = Math.max(2, Math.round((clip.width * height) / clip.height)) & ~1;
  return { videoKbps, width, height, seconds };
}

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
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState(0);
  const plan = preview(clip, targetMb);

  // Escape closes — but not mid-run, where it would leave a half-written file
  // behind with nothing saying so.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
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
        if (!cancelled && update.clipId === clip.id) setProgress(update.progress);
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
    const suggested = `${clipName(clip).replace(/\.[^.]+$/, "")} (${targetMb} MB).mp4`;
    const output = await saveDialog({
      defaultPath: suggested,
      filters: [{ name: "Video", extensions: ["mp4"] }],
      title: "Save the export as",
    });
    if (!output) return;
    setBusy(true);
    setProgress(0);
    try {
      await api.exportClip(clip.id, targetMb * MB, output);
      onClose();
    } catch {
      // The core has already raised the failure as a notice.
      setBusy(false);
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
        <h2 className="text-base font-medium">Export a copy</h2>
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

        {busy && (
          <div className="mt-4 h-px w-full bg-line">
            <div
              className="h-px bg-white transition-[width] duration-200"
              style={{ width: `${Math.round(progress * 100)}%` }}
            />
          </div>
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
            {busy ? "Exporting…" : "Export"}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
