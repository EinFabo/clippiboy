import { useMemo, useState } from "react";
import { useEngine } from "@/store";
import { Card, Pill, SectionTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { ClipPlayer } from "@/components/ClipPlayer";
import { IconArrowUpRight, IconCamera, IconScissors } from "@/components/icons";
import { clipName, formatAgo, formatBufferSeconds, formatDuration } from "@/lib/format";
import { fileUrl } from "@/lib/ipc";
import type { Route } from "@/components/NavBar";
import { SourceTrouble } from "@/components/SourceTrouble";
import { cn } from "@/lib/cn";
import { LiveDot } from "@/components/ui/LiveDot";
import { useCountUp } from "@/lib/useCountUp";

/**
 * The quality steps by name. Kept in step with `QUALITY_STEPS` in the recording
 * settings — the tile is a shortcut to that page, so it should say the same word.
 */
function qualityLabel(quality: number): string {
  if (quality < 59) return "Smallest files";
  if (quality < 66) return "Small";
  if (quality < 74) return "Balanced";
  if (quality < 82) return "High";
  return "Highest";
}

export function Dashboard({ onNavigate }: { onNavigate: (r: Route) => void }) {
  const {
    config,
    clips,
    bufferActive,
    bufferedSeconds,
    rateControl,
    detectedGame,
    toggleBuffer,
    saveClip,
    takeScreenshot,
    deleteClip,
  } = useEngine();
  const [playing, setPlaying] = useState<number | null>(null);
  // Recordings only. The player below is the video one, and "Latest clips" says
  // what it shows — screenshots have their own place in the gallery.
  const recent = useMemo(
    () => clips.filter((clip) => !clip.screenshot).slice(0, 3),
    [clips],
  );

  return (
    <div className="space-y-12">
      <header className="pt-10 pb-4 text-center">
        <h1 className="display text-5xl">ClippiBoy</h1>
        <p className="mx-auto mt-4 max-w-md text-[15px] leading-relaxed text-white/70">
          Runs in the background, listens to every audio source separately, and
          saves — after the fact — exactly the moment that was worth it.
        </p>
        <div className="mt-7 flex justify-center gap-3">
          <Button
            variant={bufferActive ? "secondary" : "primary"}
            onClick={toggleBuffer}
          >
            {bufferActive ? "Stop buffer" : "Start buffer"}
          </Button>
          <Button
            variant="secondary"
            icon={<IconScissors className="h-4 w-4" />}
            onClick={saveClip}
            disabled={!bufferActive}
          >
            Save clip
          </Button>
          {/* No `disabled` on this one: a screenshot brings its own capture
              session and does not care whether the buffer is running. */}
          <Button
            variant="secondary"
            icon={<IconCamera className="h-4 w-4" />}
            onClick={takeScreenshot}
          >
            Screenshot
          </Button>
        </div>
      </header>

      <section className="grid grid-cols-4 gap-4">
        <Stat
          label="Replay buffer"
          count={bufferActive ? bufferedSeconds : 0}
          format={(s) => (bufferActive ? formatBufferSeconds(Math.round(s)) : "off")}
          hint={`of ${formatBufferSeconds(config.buffer.seconds)}`}
          live={bufferActive}
        />
        <Stat
          label="Detected game"
          value={detectedGame ?? "none"}
          hint={
            detectedGame
              ? "Its name goes on the next clip"
              : "No game in the foreground"
          }
        />
        <Stat
          label="Recording"
          value={`${config.recording.height}p${config.recording.fps}`}
          hint={`${qualityLabel(config.recording.quality)} · ${config.recording.encoder.toUpperCase()}${
            rateControl && rateControl !== "quality" ? " · fixed bitrate" : ""
          }`}
          onClick={() => onNavigate("recording")}
        />
        <Stat
          label="Audio sources"
          count={config.sources.filter((s) => s.enabled).length}
          format={(n) => String(Math.round(n))}
          hint={`${config.sources.filter((s) => s.separateTrack).length} on their own track`}
          onClick={() => onNavigate("audio")}
        />
      </section>

      <SourceTrouble onOpenMixer={() => onNavigate("audio")} />

      <section>
        <SectionTitle
          title="Latest clips"
          action={
            <Button size="sm" variant="ghost" onClick={() => onNavigate("clips")}>
              See all
            </Button>
          }
        />
        {recent.length === 0 ? (
          <Card className="grid h-44 place-items-center text-sm text-ink-muted">
            No clips yet — start the buffer and press{" "}
            <kbd className="mx-1 rounded-inner border border-line bg-elevated px-2 py-0.5 text-xs">
              {config.saveClipHotkey}
            </kbd>
          </Card>
        ) : (
          <div className="grid grid-cols-3 gap-4">
            {recent.map((clip, index) => (
              <Card
                key={clip.id}
                interactive
                onClick={() => setPlaying(index)}
                className="cursor-pointer overflow-hidden"
              >
                <div className="relative aspect-video bg-gradient-to-br from-accent-deep/40 to-black">
                  {clip.thumbPath && (
                    <img
                      src={fileUrl(clip.thumbPath)}
                      alt=""
                      className="h-full w-full object-cover"
                    />
                  )}
                  {/* One row for both pills: a long game name truncates rather
                      than sliding under the duration. */}
                  <div className="absolute inset-x-3 bottom-3 flex items-end justify-between gap-2">
                    <Pill className="min-w-0">
                      <span className="min-w-0 truncate">{clip.game ?? "Unknown"}</span>
                    </Pill>
                    <Pill className="shrink-0">{formatDuration(clip.durationMs)}</Pill>
                  </div>
                </div>
                <div className="flex items-center justify-between p-4">
                  <div className="min-w-0">
                    <p className="truncate text-sm font-medium">
                      {clipName(clip)}
                    </p>
                    <p className="mt-1 text-xs text-ink-muted">
                      {formatAgo(clip.createdAt)}
                    </p>
                  </div>
                  <span className="grid h-8 w-8 shrink-0 place-items-center rounded-pill border border-line text-ink-muted">
                    <IconArrowUpRight className="h-4 w-4" />
                  </span>
                </div>
              </Card>
            ))}
          </div>
        )}
      </section>

      {playing !== null && recent.length > 0 && (
        <ClipPlayer
          clips={recent}
          index={Math.min(playing, recent.length - 1)}
          onIndexChange={setPlaying}
          onClose={() => setPlaying(null)}
          onDelete={deleteClip}
          onOpenMixer={() => onNavigate("audio")}
        />
      )}
    </div>
  );
}

/**
 * A number with its name. Either a finished string, or — where the value is
 * worth watching move — a raw number plus how to write it, so it can run up to
 * a new reading instead of jumping to it.
 */
function Stat(
  props: {
    label: string;
    hint: string;
    live?: boolean;
    onClick?: () => void;
  } & ({ value: string } | { count: number; format: (n: number) => string }),
) {
  const { label, hint, live, onClick } = props;
  return (
    <Card
      interactive={!!onClick}
      onClick={onClick}
      className={cn("p-5", onClick && "cursor-pointer")}
    >
      <div className="flex items-center gap-2 text-xs font-medium text-ink-muted">
        {live && <LiveDot />}
        {label}
      </div>
      <p className="display mt-3 truncate text-3xl">
        {"count" in props ? (
          <Counted value={props.count} format={props.format} />
        ) : (
          props.value
        )}
      </p>
      <p className="mt-1 text-xs text-ink-faint">{hint}</p>
    </Card>
  );
}

/** Its own component so only the number re-renders while it is running. */
function Counted({ value, format }: { value: number; format: (n: number) => string }) {
  return <>{format(useCountUp(value))}</>;
}
