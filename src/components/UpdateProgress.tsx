import type { UpdateProgress as Progress } from "@/lib/types";
import { ProgressBar } from "@/components/ui/ProgressBar";

export const mb = (bytes: number) => (bytes / 1024 / 1024).toFixed(1);

/** What the download of an update is doing, in words. */
export function updateProgressText(progress: Progress): string {
  if (progress.finished) return "Starting the installer …";
  if (progress.downloaded === 0) return "Connecting to GitHub …";
  return progress.total
    ? `Downloading … ${mb(progress.downloaded)} of ${mb(progress.total)} MB`
    : `Downloading … ${mb(progress.downloaded)} MB`;
}

/**
 * The bar under an update that is being downloaded. Without a known size it
 * stays empty rather than guessing; the text still counts up.
 */
export function UpdateProgressBar({ progress }: { progress: Progress }) {
  const share = progress.finished
    ? 1
    : progress.total
      ? progress.downloaded / progress.total
      : 0;
  return <ProgressBar share={share} className="mt-2" />;
}
