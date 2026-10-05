import { useEffect } from "react";
import { IconCheck, IconClose } from "@/components/icons";
import { ProgressBar } from "@/components/ui/ProgressBar";
import { useLinks, type LinkProgress } from "@/lib/links";

/**
 * Share links on their way up. They stand in the same column as the clips
 * going to friends (`Transfers`), bottom right — two columns there would lie
 * on top of each other.
 */
export function useLinkUploads(): LinkProgress[] {
  return Object.values(useLinks((s) => s.uploads));
}

export function UploadCard({ upload: u }: { upload: LinkProgress }) {
  const dismiss = useLinks((s) => s.dismiss);
  const over = u.stage === "done" || u.stage === "failed";
  useEffect(() => {
    if (!over) return;
    const timer = window.setTimeout(() => dismiss(u.clipId), u.stage === "failed" ? 15_000 : 5_000);
    return () => window.clearTimeout(timer);
  }, [over, u.stage, u.clipId, dismiss]);

  const headline =
    u.stage === "shrinking"
      ? "Making it fit for a link…"
      : u.stage === "uploading"
        ? "Uploading for a link…"
        : u.stage === "done"
          ? "Link copied — expires in 5 days"
          : "No link";

  return (
    <div className="rounded-card border border-line bg-surface/95 p-4 shadow-2xl backdrop-blur">
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium">{headline}</p>
          <p className="truncate text-xs text-ink-muted">{u.title}</p>
        </div>
        {u.stage === "done" && <IconCheck className="h-4 w-4 shrink-0 text-ok" />}
        {u.stage === "failed" && (
          <button
            aria-label="Dismiss"
            title="Dismiss"
            onClick={() => dismiss(u.clipId)}
            className="grid h-7 w-7 shrink-0 place-items-center rounded-pill text-ink-muted hover:bg-hover hover:text-ink"
          >
            <IconClose className="h-3.5 w-3.5" />
          </button>
        )}
      </div>
      {!over && (
        <div className="mt-3 flex items-center gap-3">
          <ProgressBar share={u.progress} className="flex-1" />
          <span className="text-xs text-ink-muted tabular-nums">{Math.floor(u.progress * 100)} %</span>
        </div>
      )}
      {u.stage === "failed" && u.error && <p className="mt-2 text-xs text-live">{u.error}</p>}
    </div>
  );
}
