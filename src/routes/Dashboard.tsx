import { useEffect, useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card, Pill, SectionTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { ClipPlayer } from "@/components/ClipPlayer";
import { ShotViewer } from "@/components/ShotViewer";
import { ShareMenuButton, copyEntries } from "@/components/clipMenu";
import { IconCamera, IconCopy, IconRecord, IconScissors } from "@/components/icons";
import { clipName, formatAgo, formatDuration, formatSize } from "@/lib/format";
import { fileUrl } from "@/lib/ipc";
import type { Route } from "@/components/NavBar";
import { SourceTrouble } from "@/components/SourceTrouble";
import { cn } from "@/lib/cn";
import { hotkeyLabel, shownHotkey } from "@/lib/hotkeyText";
import { LiveDot } from "@/components/ui/LiveDot";
import { ProgressBar } from "@/components/ui/ProgressBar";
import {
  audioSummary,
  bufferStatus,
  justNowLeft,
  latestItems,
  saveStep,
  videoSummary,
} from "@/lib/overview";
import type { Clip } from "@/lib/types";

/** A hotkey as a small key chip; nothing when none is bound. */
function Key({ binding }: { binding: ReturnType<typeof shownHotkey> }) {
  if (!binding.value) return null;
  return (
    <kbd className="ml-0.5 rounded-[6px] border border-current px-1.5 py-px font-mono text-[11px] font-normal opacity-70">
      {hotkeyLabel(binding.value, binding.family)}
    </kbd>
  );
}

export function Dashboard({ onNavigate }: { onNavigate: (r: Route) => void }) {
  // `useShallow`, siehe `SourceTrouble`.
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
    fileClip,
    recording,
    recordingSeconds,
    recordingBytes,
    recordingSaving,
    recordingProgress,
    toggleRecording,
    screenFallback,
  } = useEngine(
    useShallow((s) => ({
      config: s.config,
      clips: s.clips,
      bufferActive: s.bufferActive,
      bufferedSeconds: s.bufferedSeconds,
      rateControl: s.rateControl,
      detectedGame: s.detectedGame,
      toggleBuffer: s.toggleBuffer,
      saveClip: s.saveClip,
      takeScreenshot: s.takeScreenshot,
      deleteClip: s.deleteClip,
      fileClip: s.fileClip,
      recording: s.recording,
      recordingSeconds: s.recordingSeconds,
      recordingBytes: s.recordingBytes,
      recordingSaving: s.recordingSaving,
      recordingProgress: s.recordingProgress,
      toggleRecording: s.toggleRecording,
      screenFallback: s.screenFallback,
    })),
  );
  const saving = recordingSaving || recordingProgress !== null;
  // Named as the pad last used goes, else the keyboard's.
  const saveKey = shownHotkey(config, "saveClip");
  const screenshotKey = shownHotkey(config, "screenshot");
  const recordKey = shownHotkey(config, "record");
  const status = bufferStatus({
    bufferActive,
    game: detectedGame,
    bufferedSeconds,
    bufferSeconds: config.buffer.seconds,
  });
  const off = status.state === "off";
  // Of every kind: the Overview shows what just happened, not only clips.
  const recent = useMemo(() => latestItems(clips), [clips]);
  // "Just now" has to go by itself: a timer to the minute, and a slow tick so
  // the "5m ago" texts do not stand still.
  const [now, setNow] = useState(() => Date.now());
  const newest = recent[0];
  const newestAt = newest?.createdAt;
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(id);
  }, []);
  useEffect(() => {
    if (newestAt === undefined) return;
    const left = justNowLeft(newestAt, Date.now());
    setNow(Date.now());
    if (left === null) return;
    const id = setTimeout(() => setNow(Date.now()), left + 50);
    return () => clearTimeout(id);
  }, [newestAt]);
  const newestFresh = newestAt !== undefined && justNowLeft(newestAt, now) !== null;
  /**
   * The player's clips, by id, as they stood when it opened — and the one on
   * screen. By position, as before, a clip saved meanwhile (hotkey, console,
   * a friend's) pushed the list along: the same index named another clip, and
   * the trim and mix set on the one being watched were gone.
   */
  const [playing, setPlaying] = useState<{
    ids: string[];
    shown: string;
    /** Opened in the player — filed when it closes, as the gallery does. */
    seen: string[];
  } | null>(null);
  const playlist = useMemo(
    () =>
      (playing?.ids ?? []).flatMap((id) => clips.find((clip) => clip.id === id) ?? []),
    [playing, clips],
  );
  const shownIndex = playlist.findIndex((clip) => clip.id === playing?.shown);
  // Deleted elsewhere (the console, say): nothing left to show.
  useEffect(() => {
    if (playing && shownIndex < 0) setPlaying(null);
  }, [playing, shownIndex]);

  const open = (clip: Clip) =>
    setPlaying({ ids: recent.map((c) => c.id), shown: clip.id, seen: [clip.id] });
  const shown = playlist[shownIndex];
  const thinned = (index: number) =>
    setPlaying((was) => {
      // Read off the ids in the update, not off this render's list: a delete
      // right before has already taken its id out. Thinned to the items still
      // there, as `playlist` is.
      const id = was?.ids.filter((other) => clips.some((clip) => clip.id === other))[index];
      return was && id ? { ...was, shown: id, seen: [...was.seen, id] } : was;
    });
  const closePlayer = () => {
    // A heart set in the player moves the file into the favourites — once the
    // player has let go of it, as in the gallery.
    for (const id of new Set(playing?.seen ?? [])) {
      if (clips.some((clip) => clip.id === id)) void fileClip(id);
    }
    setPlaying(null);
  };
  const deleteFromPlayer = (id: string) => {
    setPlaying((was) => was && { ...was, ids: was.ids.filter((other) => other !== id) });
    void deleteClip(id);
  };

  return (
    <div className="space-y-6">
      <header className="pt-10">
        <h1 className="display text-4xl">ClippiBoy</h1>
      </header>

      <Card
        className={cn(
          "grid grid-cols-[1fr_auto] items-center gap-x-6 gap-y-3.5 px-6 py-5",
          off && "border-warn/40 bg-warn/10",
        )}
      >
        <div className="flex min-w-0 items-center gap-3.5">
          {off ? (
            <span className="h-2.5 w-2.5 shrink-0 rounded-pill bg-warn" />
          ) : (
            <LiveDot className="h-2.5 w-2.5" />
          )}
          <div className="min-w-0">
            <p className={cn("display truncate text-xl", off && "text-warn")}>{status.title}</p>
            {status.sub && <p className="mt-0.5 text-[13px] text-ink-muted tabular-nums">{status.sub}</p>}
          </div>
        </div>
        <div className="flex gap-2.5">
          {off ? (
            <Button variant="primary" onClick={toggleBuffer}>
              Start buffer
            </Button>
          ) : (
            <>
              <Button
                variant="primary"
                icon={<IconScissors className="h-4 w-4" />}
                onClick={saveClip}
              >
                Save clip
                <Key binding={saveKey} />
              </Button>
              <Button onClick={toggleBuffer}>Stop</Button>
            </>
          )}
        </div>
        {!off && <ProgressBar share={status.fill} className="col-span-full h-1.5" />}
      </Card>

      {/* Until now only the recording page said so, and nobody has that open
          while playing. The core moves the capture back by itself once the
          screen returns — this is for the minutes in between. */}
      {screenFallback && (
        <Card className="flex items-start gap-4 border-warn/40 bg-warn/10 p-4">
          <p className="min-w-0 flex-1 text-[13px] leading-relaxed text-ink-muted">
            <span className="font-medium text-warn">The screen you picked is not connected</span>{" "}
            — recording <span className="text-ink">{screenFallback}</span> for now. It moves
            back on its own once the screen is there again.
          </p>
          <Button size="sm" className="shrink-0" onClick={() => onNavigate("recording")}>
            Choose screen
          </Button>
        </Card>
      )}

      <SourceTrouble onOpenMixer={() => onNavigate("audio")} />

      <div>
        <div className="flex flex-wrap items-center gap-2.5">
          {/* No `disabled` on this one: a screenshot brings its own capture
              session and does not care whether the buffer is running. */}
          <Button icon={<IconCamera className="h-4 w-4" />} onClick={takeScreenshot}>
            Screenshot
            <Key binding={screenshotKey} />
          </Button>
          {/* Works without the buffer too: it brings the capture up itself. */}
          <Button
            icon={<IconRecord className={cn("h-4 w-4", recording ? "text-live" : "text-live/80")} />}
            onClick={toggleRecording}
            disabled={saving}
          >
            {saving
              ? `Saving recording… ${Math.round((recordingProgress ?? 0) * 100)} %`
              : recording
                ? `Stop · ${formatDuration(recordingSeconds * 1000)}`
                : "Record"}
            {!saving && !recording && <Key binding={recordKey} />}
          </Button>
          <p className="ml-auto text-[13px] text-ink-muted">
            <button className="hover:text-ink" onClick={() => onNavigate("recording")}>
              {videoSummary(config.recording, !!rateControl && rateControl !== "quality")}
            </button>
            {" · "}
            <button className="hover:text-ink" onClick={() => onNavigate("audio")}>
              {audioSummary(config.sources.filter((src) => src.enabled).length)}
              <span className="ml-1 text-ink">›</span>
            </button>
          </p>
        </div>
        {saving && <ProgressBar share={recordingProgress ?? 0} className="mt-3 w-72" />}
        {recording && (
          <p className="mt-3 text-xs text-ink-muted tabular-nums">
            ~{formatSize(recordingBytes)}
            {recordKey.value && (
              <>
                {" "}·{" "}
                <kbd className="rounded-inner border border-line bg-elevated px-1.5 py-0.5">
                  {hotkeyLabel(recordKey.value, recordKey.family)}
                </kbd>{" "}
                stops it
              </>
            )}
          </p>
        )}
      </div>

      <section className="pt-4">
        <SectionTitle
          title="Latest"
          action={
            <Button size="sm" variant="ghost" onClick={() => onNavigate("clips")}>
              See all
            </Button>
          }
        />
        {recent.length === 0 ? (
          <Card className="p-6">
            <ol className="flex flex-wrap items-center gap-x-8 gap-y-3 text-sm">
              {["Start the buffer", "Play", saveStep(saveKey.value ? hotkeyLabel(saveKey.value, saveKey.family) : null)].map(
                (step, i) => (
                  <li key={step} className="flex items-center gap-3">
                    <span className="grid h-6 w-6 place-items-center rounded-pill border border-line text-xs text-ink-muted">
                      {i + 1}
                    </span>
                    {step}
                  </li>
                ),
              )}
            </ol>
          </Card>
        ) : (
          <div className="grid grid-cols-3 gap-4">
            {recent.map((clip, i) => (
              <LatestCard
                key={clip.id}
                clip={clip}
                ago={formatAgo(clip.createdAt, now)}
                fresh={i === 0 && newestFresh}
                quick={i === 0}
                onOpen={() => open(clip)}
              />
            ))}
          </div>
        )}
      </section>

      {playing !== null &&
        shownIndex >= 0 &&
        (shown?.screenshot ? (
          <ShotViewer
            clips={playlist}
            index={shownIndex}
            onIndexChange={thinned}
            onClose={closePlayer}
            onDelete={deleteFromPlayer}
          />
        ) : (
          <ClipPlayer
            clips={playlist}
            index={shownIndex}
            onIndexChange={thinned}
            onClose={closePlayer}
            onDelete={deleteFromPlayer}
            onOpenMixer={() => onNavigate("audio")}
          />
        ))}
    </div>
  );
}

/**
 * One of the newest items, in the gallery's look: thumbnail with its pills, the
 * name and a line of facts, and two buttons. The newest has its quickest copy
 * action first; the others Open.
 */
function LatestCard({
  clip,
  ago,
  fresh,
  quick,
  onOpen,
}: {
  clip: Clip;
  ago: string;
  fresh: boolean;
  quick: boolean;
  onOpen: () => void;
}) {
  const copies = quick ? copyEntries(clip) : [];
  const copy = copies.find((e) => e.kind === "item" && e.label === "Copy for Discord") ?? copies[0];
  const kind = clip.screenshot ? "Screenshot" : clip.recording ? "Recording" : null;
  return (
    <Card interactive className="group overflow-hidden">
      <button
        onClick={onOpen}
        aria-label={`${clip.screenshot ? "Open" : "Play"} ${clip.game ?? kind ?? "clip"}`}
        className="relative block aspect-video w-full bg-gradient-to-br from-accent-deep/40 to-black"
      >
        {clip.thumbPath && (
          <img
            src={`${fileUrl(clip.thumbPath)}?v=${clip.sizeBytes}`}
            loading="lazy"
            decoding="async"
            alt=""
            className="h-full w-full object-cover"
          />
        )}
        {/* One row for both pills: a long game name truncates rather than
            sliding under the duration. */}
        <span className="absolute inset-x-3 bottom-3 flex items-end justify-between gap-2">
          <Pill className="min-w-0">
            <span className="min-w-0 truncate">{clip.game ?? "Unknown"}</span>
          </Pill>
          {clip.screenshot ? (
            <Pill title="Screenshot" className="shrink-0">
              <IconCamera className="h-3 w-3" />
            </Pill>
          ) : clip.recording ? (
            <Pill title="Recording" className="shrink-0">
              <IconRecord className="h-3 w-3 text-live" />
              {formatDuration(clip.durationMs)}
            </Pill>
          ) : (
            <Pill className="shrink-0">{formatDuration(clip.durationMs)}</Pill>
          )}
        </span>
        {fresh && (
          <span className="absolute top-3 right-3 inline-flex h-[22px] items-center rounded-pill bg-accent px-2.5 text-[11px] font-semibold">
            Just now
          </span>
        )}
      </button>
      <div className="p-4">
        <p className="truncate text-sm font-medium">{clipName(clip)}</p>
        <p className="mt-1 truncate text-xs text-ink-muted">
          {kind ?? clip.game ?? "Clip"} · {ago}
        </p>
        <div className="mt-3 flex gap-2">
          {copy && copy.kind === "item" ? (
            <Button
              size="sm"
              icon={<IconCopy className="h-4 w-4" />}
              disabled={copy.disabled}
              onClick={copy.onSelect}
            >
              {copy.label}
            </Button>
          ) : (
            <Button size="sm" onClick={onOpen}>
              Open
            </Button>
          )}
          <ShareMenuButton clip={clip} variant="secondary" />
        </div>
      </div>
    </Card>
  );
}
