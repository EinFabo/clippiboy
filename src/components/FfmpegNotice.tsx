import { useEngine } from "@/store";
import { api } from "@/lib/ipc";
import { Card } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { ProgressBar } from "@/components/ui/ProgressBar";
import { mb } from "./UpdateProgress";

/**
 * ffmpeg is fetched once on first start instead of riding along in every
 * update. Until it is in, the buffer runs but clips cannot be written — which
 * is worth saying on every page, not only when a save fails.
 */
export function FfmpegNotice() {
  const ffmpeg = useEngine((s) => s.ffmpeg);

  // Looking takes a split second; showing a card for it would only flicker.
  if (ffmpeg.state === "ready" || ffmpeg.state === "checking") return null;

  let text = "";
  let share = 0;
  if (ffmpeg.state === "downloading") {
    text = ffmpeg.total
      ? `Downloading … ${mb(ffmpeg.downloaded)} of ${mb(ffmpeg.total)} MB`
      : `Downloading … ${mb(ffmpeg.downloaded)} MB`;
    share = ffmpeg.total ? ffmpeg.downloaded / ffmpeg.total : 0;
  } else if (ffmpeg.state === "unpacking") {
    text = "Unpacking and checking …";
    share = 1;
  } else if (ffmpeg.state === "failed") {
    text = `${ffmpeg.error} — trying again in ${retryIn(ffmpeg.retryInSecs)}.`;
  }

  return (
    <Card
      className={
        ffmpeg.state === "failed"
          ? "mb-6 flex items-center gap-4 border-live/40 bg-live/10 p-4"
          : "mb-6 flex items-center gap-4 border-accent/40 bg-accent/10 p-4"
      }
    >
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium text-ink">
          {ffmpeg.state === "failed"
            ? "ffmpeg could not be downloaded"
            : "Setting up ffmpeg — one time only"}
        </p>
        <p className="mt-0.5 text-xs text-ink-muted">{text}</p>
        {ffmpeg.state !== "failed" && <ProgressBar share={share} className="mt-2" />}
        <p className="mt-1 text-xs text-ink-faint">
          The buffer already records; clips can be saved once this is done.
        </p>
      </div>
      {ffmpeg.state === "failed" && (
        <Button size="sm" variant="primary" onClick={() => void api.retryFfmpeg()}>
          Retry now
        </Button>
      )}
    </Card>
  );
}

function retryIn(secs: number): string {
  return secs < 60 ? `${secs} s` : `${Math.round(secs / 60)} min`;
}
