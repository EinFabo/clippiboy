import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/Button";
import { Pill } from "@/components/ui/Card";
import { ClipEditor, type Trim } from "@/components/ClipEditor";
import { useClipMenu } from "@/components/clipMenu";
import { IconHeart, IconTrash } from "@/components/icons";
import { useEngine } from "@/store";
import { api, events, fileUrl, inTauri } from "@/lib/ipc";
import { formatAgo, formatSize } from "@/lib/format";
import { useClipMix } from "@/lib/useClipMix";
import { cn } from "@/lib/cn";
import type { Clip, ClipEdit } from "@/lib/types";

interface Props {
  clips: Clip[];
  index: number;
  onIndexChange: (index: number) => void;
  onClose: () => void;
  onDelete: (id: string) => void;
  /** Switch to the audio mixer — that is where the separated tracks come from. */
  onOpenMixer: () => void;
}

/** One frame at 30 fps. Enough to set a mark cleanly. */
const FRAME = 1 / 30;

/**
 * Full-bleed player over the gallery, with the editing pane beside it.
 *
 * There is no edit mode: name, audio tracks and trim lie open, and whatever is
 * set the core remembers on the clip. The video track always stays untouched in
 * the process — only the audio gets baked in, and only at the press of a button.
 *
 * A note on multiple audio tracks: WebView2 does not provide
 * `HTMLMediaElement.audioTracks`, so the video element only ever plays back the
 * main mix. The core extracts the remaining tracks individually and `useClipMix`
 * runs them along in sync — that is the only way a mix can be judged at all.
 */
export function ClipPlayer({
  clips,
  index,
  onIndexChange,
  onClose,
  onDelete,
  onOpenMixer,
}: Props) {
  const clip = clips[index];
  const video = useRef<HTMLVideoElement>(null);
  const frame = useRef<HTMLDivElement>(null);
  const applyClipEdit = useEngine((state) => state.applyClipEdit);
  const setFavorite = useEngine((state) => state.setFavorite);
  const clipMenu = useClipMenu();
  const restoreClipOriginal = useEngine((state) => state.restoreClipOriginal);

  const [playing, setPlaying] = useState(false);
  const [time, setTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [volume, setVolume] = useState(1);
  const [muted, setMuted] = useState(false);
  const [broken, setBroken] = useState(false);
  const [trim, setTrim] = useState<Trim>({ start: 0, end: 0 });
  const [waveform, setWaveform] = useState<string | undefined>(undefined);
  const [saving, setSaving] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const [writeProgress, setWriteProgress] = useState<number | null>(null);
  const [justSaved, setJustSaved] = useState(false);
  // Counts the video element's restarts. Serves as a `key`: after saving, the
  // player gets a fresh element rather than one whose source we pulled out from
  // under it. The audio tracks re-attach to it as well.
  const [reload, setReload] = useState(0);
  /** Where to jump back to after reloading. */
  const resumeAt = useRef<{ time: number; playing: boolean } | null>(null);
  /** Failed attempts at loading the current file. */
  const loadFailures = useRef(0);

  // The video element's volume belongs to the mixer: track 0 is the main mix,
  // and that has to match the other tracks.
  const mix = useClipMix(clip, video, muted ? 0 : volume, reload);

  // The success message belongs to exactly one clip — when paging through, it
  // would otherwise be a claim about a clip nobody saved.
  useEffect(() => {
    setJustSaved(false);
  }, [clip?.id]);

  /** Delete and move on within the player — if the last clip is gone, there is
      nothing left to show. */
  const removeClip = useCallback(() => {
    if (!clip) return;
    onDelete(clip.id);
    if (clips.length <= 1) onClose();
    else onIndexChange(Math.min(index, clips.length - 2));
  }, [clip, clips.length, index, onClose, onDelete, onIndexChange]);

  const step = useCallback(
    (delta: number) => {
      const next = index + delta;
      if (next >= 0 && next < clips.length) onIndexChange(next);
    },
    [clips.length, index, onIndexChange],
  );

  // Reset everything when switching clips.
  useEffect(() => {
    setBroken(false);
    // Otherwise the next clip would jump to where the previous one stood.
    resumeAt.current = null;
    loadFailures.current = 0;
    setTime(0);
    setDuration(0);
    setTrim({ start: 0, end: 0 });
    setWaveform(undefined);
  }, [clip?.id]);

  // The picture of the audio track for the timeline. It arrives later than the
  // video and then simply appears — if it is missing, the bar stays as it is.
  const clipId = clip?.id;
  // The core reports how far along it is. Without this the button would stand
  // there for minutes with no sign of life on a cut at the start.
  useEffect(() => {
    if (!clipId || !inTauri) return;
    let unlisten: (() => void) | undefined;
    void events
      .onClipProgress((update) => {
        if (update.clipId === clipId) setWriteProgress(update.progress);
      })
      .then((off) => {
        unlisten = off;
      });
    return () => unlisten?.();
  }, [clipId]);

  useEffect(() => {
    if (!clipId || !inTauri) return;
    let cancelled = false;
    api
      .clipWaveform(clipId)
      .then((path) => !cancelled && setWaveform(fileUrl(path)))
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [clipId]);

  // While playing, the position is drawn without React: the frame loop writes
  // bar, handle and clock straight into the DOM.
  //
  // This used to be a `setTime` per frame, i.e. sixty renders a second. WebView2
  // renders React and the video on the same thread and dropped video frames over
  // it — the clip looked like it stuttered and ran away from the audio. Measured
  // afterwards, the file is impeccable: 1973 of 1975 frame intervals exactly
  // 17 ms, video and audio 18 ms apart.
  const fill = useRef<HTMLDivElement>(null);
  const knob = useRef<HTMLDivElement>(null);
  const clockLabel = useRef<HTMLSpanElement>(null);

  const paint = useCallback((seconds: number, total: number) => {
    const ratio = total > 0 ? Math.min(1, Math.max(0, seconds / total)) : 0;
    const percent = `${ratio * 100}%`;
    if (fill.current) fill.current.style.width = percent;
    if (knob.current) knob.current.style.left = percent;
    if (clockLabel.current) clockLabel.current.textContent = clock(seconds);
  }, []);

  // `timeupdate` fires only about four times a second — the bar would visibly
  // jump. While playing, a frame loop reads the position straight off the
  // element. The trim is enforced here along the way.
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    const tick = () => {
      const element = video.current;
      if (element) {
        // Stay inside the selection: at the set end, back to its start — while
        // trimming you want to hear that spot several times, not the rest of the
        // clip. If playback ran in from before the selection, it is pulled to the
        // start as well.
        if (trim.end > trim.start) {
          const at = element.currentTime;
          if (at >= trim.end || at < trim.start - 0.25) {
            element.currentTime = trim.start;
          }
        }
        paint(element.currentTime, element.duration);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing, trim.start, trim.end, paint]);

  const toggle = useCallback(() => {
    const element = video.current;
    if (!element) return;
    if (!element.paused) {
      element.pause();
      return;
    }
    // If the playhead sits outside the selection, playback starts at its start —
    // otherwise the first thing to run would be exactly what was cut away.
    if (trim.end > trim.start) {
      const at = element.currentTime;
      if (at < trim.start - 0.05 || at >= trim.end - 0.05) {
        element.currentTime = trim.start;
      }
    }
    void element.play();
  }, [trim.start, trim.end]);

  const seek = useCallback((seconds: number) => {
    const element = video.current;
    if (!element || !Number.isFinite(element.duration)) return;
    jump(element, element.currentTime + seconds);
  }, []);

  /** Jump to a fixed position and pull the display along right away. */
  const seekTo = useCallback((seconds: number) => {
    const element = video.current;
    if (!element || !Number.isFinite(element.duration)) return;
    jump(element, seconds);
  }, []);

  const fullscreen = useCallback(() => {
    if (document.fullscreenElement) void document.exitFullscreen();
    else void frame.current?.requestFullscreen();
  }, []);

  /** Put the selection's start or end at the current position. */
  const mark = useCallback((which: "start" | "end") => {
    const element = video.current;
    if (!element) return;
    const at = element.currentTime;
    setTrim((current) =>
      which === "start"
        ? { start: Math.min(at, current.end - 0.5), end: current.end }
        : { start: current.start, end: Math.max(at, current.start + 0.5) },
    );
  }, []);

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      // In text fields, space and letters stay what they are.
      const target = event.target;
      if (
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement
      ) {
        return;
      }
      const handlers: Record<string, () => void> = {
        " ": toggle,
        k: toggle,
        // Smaller steps with shift — 5 s regularly jumps past the spot you are
        // looking for.
        ArrowRight: () => seek(event.shiftKey ? 1 : 5),
        ArrowLeft: () => seek(event.shiftKey ? -1 : -5),
        // Single frames, to set start and end precisely.
        ".": () => seek(FRAME),
        ",": () => seek(-FRAME),
        ArrowUp: () => setVolume((v) => Math.min(1, v + 0.1)),
        ArrowDown: () => setVolume((v) => Math.max(0, v - 0.1)),
        Home: () => seekTo(trim.start),
        End: () => seekTo(Math.max(trim.start, trim.end - FRAME)),
        m: () => setMuted((m) => !m),
        f: fullscreen,
        i: () => mark("start"),
        o: () => mark("end"),
        n: () => step(1),
        p: () => step(-1),
        Escape: () => (document.fullscreenElement ? undefined : onClose()),
      };
      const handler = handlers[event.key] ?? handlers[event.key.toLowerCase()];
      if (!handler) return;
      event.preventDefault();
      handler();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toggle, seek, seekTo, fullscreen, mark, step, onClose, trim.start, trim.end]);

  const base = clip ? fileUrl(clip.path) : undefined;
  // After saving the file is a different one — usually shorter, because the
  // audio was recomputed. Under the same address the WebView keeps serving the
  // video element from its cache and issues range requests past the new end of
  // file. The element reports that as an error, and the player then claims the
  // file is gone. For the file path Tauri only evaluates `uri().path()`, so the
  // suffix does no harm there.
  const source = base && reload > 0 ? `${base}?v=${reload}` : base;
  const current: ClipEdit = {
    startMs: Math.round(trim.start * 1000),
    endMs: Math.round(trim.end * 1000),
    tracks: mix.toRequest(),
  };
  const dirty =
    duration > 0 &&
    mix.tracks.length > 0 &&
    !sameEdit(current, savedEdit(clip?.edit ?? null, duration, current.tracks));

  /**
   * Write the mix into the file.
   *
   * The player has to let go of the file for that: Windows will not replace an
   * open file, and the core rewrites the clip. Position and playback are restored
   * afterwards.
   */
  async function save() {
    if (!clip || saving || restoring) return;
    const element = video.current;
    // After a real cut the file is shorter and starts elsewhere — the same number
    // of seconds would then sit past the new end. The offset is exactly the start
    // that was cut away.
    resumeAt.current = {
      time: Math.max(0, (element?.currentTime ?? 0) - trim.start),
      playing: element ? !element.paused : false,
    };

    setSaving(true);
    setWriteProgress(0);
    setJustSaved(false);
    // Let go of the file: Windows will not replace a file that is still open.
    element?.pause();
    element?.removeAttribute("src");
    element?.load();
    try {
      await applyClipEdit(clip.id, current.startMs, current.endMs, current.tracks);
      setJustSaved(true);
    } catch {
      // The store has already raised the error as a notice. All that counts here
      // is that the player gets a playable element back below.
    } finally {
      setSaving(false);
      setWriteProgress(null);
      // A fresh element instead of the detached one. That is the only route that
      // does not depend on whatever state the old one is in.
      setBroken(false);
      setReload((n) => n + 1);
    }
  }

  /**
   * Undo the trim. Like [save]: release the file first, then a fresh video
   * element — the file underneath is a different and longer one.
   */
  async function restoreOriginal() {
    if (!clip || saving || restoring) return;
    const element = video.current;
    // The position you stand at sits later in the original by the start that was
    // cut away.
    resumeAt.current = {
      time: (element?.currentTime ?? 0) + (clip.original?.startMs ?? 0) / 1000,
      playing: false,
    };

    setRestoring(true);
    setWriteProgress(0);
    setJustSaved(false);
    element?.pause();
    element?.removeAttribute("src");
    element?.load();
    try {
      await restoreClipOriginal(clip.id);
    } catch {
      // The notice is already in the store.
    } finally {
      setRestoring(false);
      setWriteProgress(null);
      setBroken(false);
      setReload((n) => n + 1);
    }
  }

  // A new state makes the success message obsolete.
  const showSaved = justSaved && !dirty;

  if (!clip) return null;

  const progress = duration > 0 ? (time / duration) * 100 : 0;

  return (
    <div
      className="fixed inset-0 z-50 flex flex-col bg-black/80 backdrop-blur-xl"
      onClick={onClose}
    >
      {/* The clip's name lives in the editing pane and is editable there — up
          here it would stand a second time, but untouchable. */}
      <header className="flex shrink-0 items-center justify-between gap-6 px-8 pt-6 pb-4">
        <div className="flex min-w-0 items-center gap-2" onClick={(e) => e.stopPropagation()}>
          <p className="truncate text-xs text-ink-muted">
            {clip.game ?? "Unknown game"} · {formatAgo(clip.createdAt)} ·{" "}
            {formatSize(clip.sizeBytes)}
          </p>
          <Pill className="bg-white/10">{clip.height}p</Pill>
        </div>
        <button
          aria-label="Close player"
          onClick={onClose}
          className="grid h-9 w-9 shrink-0 place-items-center rounded-pill border border-line
            text-ink-muted transition-colors hover:bg-elevated hover:text-ink"
        >
          <svg viewBox="0 0 14 14" className="h-3.5 w-3.5" stroke="currentColor" strokeWidth="1.4">
            <path d="M3.5 3.5l7 7M10.5 3.5l-7 7" />
          </svg>
        </button>
      </header>

      <div
        className="flex min-h-0 flex-1 gap-4 px-8"
        onClick={(e) => e.stopPropagation()}
      >
        <div
          ref={frame}
          onContextMenu={(event) =>
            clip && clipMenu(event, clip, { onDelete: removeClip })
          }
          className="relative min-h-0 min-w-0 flex-1 overflow-hidden rounded-card bg-black"
        >
          {broken || !source ? (
            <div className="grid h-full place-items-center px-8 text-center">
              <div>
                <p className="text-sm font-medium">File not found</p>
                <p className="mx-auto mt-2 max-w-md text-xs text-ink-muted">
                  {inTauri
                    ? `The file at ${clip.path} cannot be opened — it was probably moved or deleted outside of ClippiBoy.`
                    : "There are no real clips in browser mode."}
                </p>
              </div>
            </div>
          ) : (
            <video
              // A new element after saving — see `reload`.
              key={reload}
              ref={video}
              src={source}
              autoPlay
              className="h-full w-full bg-black object-contain"
              onClick={toggle}
              onDoubleClick={fullscreen}
              onPlay={() => setPlaying(true)}
              // On pause React takes the position back over — otherwise it would
              // jump back on the next render to where it stood before playback.
              onPause={(e) => {
                setPlaying(false);
                setTime(e.currentTarget.currentTime);
              }}
              onTimeUpdate={(e) => {
                // Only relevant for the paused state and for seeking now.
                if (e.currentTarget.paused) setTime(e.currentTarget.currentTime);
              }}
              onSeeked={(e) => setTime(e.currentTarget.currentTime)}
              onLoadedMetadata={(e) => {
                const element = e.currentTarget;
                loadFailures.current = 0;
                const length = element.duration;
                setDuration(length);
                const restored = Number.isFinite(length)
                  ? restoreTrim(clip.edit, length)
                  : null;
                if (restored) setTrim(restored);

                // After saving, carry on where you were.
                const resume = resumeAt.current;
                if (resume) {
                  resumeAt.current = null;
                  element.currentTime = resume.time;
                  if (resume.playing) void element.play();
                  else element.pause();
                } else if (restored && restored.start > 0.05) {
                  // A trimmed clip starts at its mark, not at zero — otherwise
                  // the first thing you see is what was cut away.
                  element.currentTime = restored.start;
                }
              }}
              onEnded={(e) => {
                setPlaying(false);
                setTime(e.currentTarget.currentTime);
              }}
              // An element with no source cannot fail on the file — that is the
              // deliberate release during save. A flag for it would be a race
              // against this event, which arrives later; asking the source itself
              // cannot go wrong.
              onError={(e) => {
                if (!e.currentTarget.getAttribute("src")) return;
                loadFailures.current += 1;
                // Just replaced: a single failed attempt does not yet mean the
                // file is gone. Try once more with a fresh address before the
                // player claims that.
                if (loadFailures.current < 2) {
                  setReload((n) => n + 1);
                  return;
                }
                setBroken(true);
              }}
            />
          )}
        </div>

        <ClipEditor
          clip={clip}
          duration={duration}
          tracks={mix.tracks}
          mix={mix.mix}
          mixTouched={mix.touched}
          loadingTracks={mix.loading}
          onTrack={mix.setTrack}
          onResetMix={mix.reset}
          trim={trim}
          onTrim={setTrim}
          onMark={mark}
          separateTracks={mix.separate}
          dirty={dirty}
          progress={writeProgress}
          onRestore={() => void restoreOriginal()}
          restoring={restoring}
          saving={saving}
          justSaved={showSaved}
          onSave={save}
          onOpenMixer={onOpenMixer}
        />
      </div>

      <footer
        className="shrink-0 space-y-3 px-8 pt-4 pb-6"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-4">
          <button
            aria-label={playing ? "Pause" : "Play"}
            onClick={toggle}
            disabled={broken}
            className="grid h-11 w-11 shrink-0 place-items-center rounded-pill bg-white text-black
              transition-transform active:scale-95 disabled:opacity-40"
          >
            {playing ? (
              <svg viewBox="0 0 24 24" className="h-4 w-4" fill="currentColor">
                <rect x="6.5" y="5" width="3.6" height="14" rx="1.2" />
                <rect x="13.9" y="5" width="3.6" height="14" rx="1.2" />
              </svg>
            ) : (
              <svg viewBox="0 0 24 24" className="h-4 w-4 translate-x-[1px]" fill="currentColor">
                <path d="M7.5 5.2 19 12 7.5 18.8V5.2Z" />
              </svg>
            )}
          </button>

          <Scrubber
            progress={progress}
            fillRef={fill}
            knobRef={knob}
            duration={duration}
            trim={trim}
            waveform={waveform}
            onSeek={(ratio) => seekTo(ratio * duration)}
            onTrim={setTrim}
          />

          <span className="shrink-0 font-mono text-xs text-ink-muted tabular-nums">
            <span ref={clockLabel}>{clock(time)}</span> / {clock(duration)}
          </span>

          <Volume
            value={muted ? 0 : volume}
            onChange={(v) => {
              setVolume(v);
              setMuted(v === 0);
            }}
            onToggleMute={() => setMuted((m) => !m)}
          />

          <button
            aria-label="Fullscreen"
            onClick={fullscreen}
            className="grid h-9 w-9 shrink-0 place-items-center rounded-pill text-ink-muted
              transition-colors hover:bg-elevated hover:text-ink"
          >
            <svg viewBox="0 0 24 24" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.9" strokeLinecap="round" strokeLinejoin="round">
              <path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5" />
            </svg>
          </button>
        </div>

        <div className="flex items-center gap-2">
          {/* The file only moves into the favorites folder on close — mid
              playback it would be pulled out from under the player. The gallery
              takes care of it. */}
          <Button
            size="sm"
            variant={clip.favorite ? "primary" : "secondary"}
            aria-pressed={clip.favorite}
            icon={
              <IconHeart
                filled={clip.favorite}
                className={cn("h-4 w-4", !clip.favorite && "text-live")}
              />
            }
            onClick={() => void setFavorite(clip.id, !clip.favorite)}
          >
            {clip.favorite ? "Favorite" : "Add to favorites"}
          </Button>
          <Button
            size="sm"
            variant="secondary"
            onClick={() => inTauri && api.revealClip(clip.id)}
          >
            Show in folder
          </Button>
          <Button
            size="sm"
            variant="danger"
            icon={<IconTrash className="h-4 w-4" />}
            onClick={removeClip}
          >
            Delete
          </Button>

          <span className="ml-auto text-xs text-ink-faint">
            Space · ←/→ 5 s, 1 s with shift · ,/. single frame · I/O start
            &amp; end · M mute · F fullscreen
          </span>

          <div className="flex items-center gap-1">
            <Step label="Previous clip" disabled={index === 0} onClick={() => step(-1)}>
              ‹
            </Step>
            <span className="w-16 text-center text-xs text-ink-muted tabular-nums">
              {index + 1} / {clips.length}
            </span>
            <Step
              label="Next clip"
              disabled={index >= clips.length - 1}
              onClick={() => step(1)}
            >
              ›
            </Step>
          </div>
        </div>
      </footer>
    </div>
  );
}

/** Seeks, keeping the position within the file's bounds. */
function jump(element: HTMLVideoElement, seconds: number) {
  element.currentTime = Math.min(Math.max(seconds, 0), element.duration);
}

/**
 * The stored trim, clamped to the actual length. With no stored state the whole
 * clip is selected.
 */
function restoreTrim(edit: ClipEdit | null, duration: number): Trim {
  if (!edit) return { start: 0, end: duration };
  const start = Math.min(Math.max(edit.startMs / 1000, 0), duration);
  const end = Math.min(Math.max(edit.endMs / 1000, start), duration);
  return { start, end: end > start ? end : duration };
}

/**
 * The state to be saved — or `null` when nothing is set. An untouched clip should
 * get no record, otherwise every clip that was merely viewed would count as
 * edited.
 */
/**
 * The state currently sitting in the file. A clip with no stored state is the
 * whole clip with untouched sliders.
 */
function savedEdit(
  edit: ClipEdit | null,
  duration: number,
  tracks: ClipEdit["tracks"],
): ClipEdit {
  return (
    edit ?? {
      startMs: 0,
      endMs: Math.round(duration * 1000),
      tracks: tracks.map((track) => ({ ...track, gainDb: 0, muted: false })),
    }
  );
}

/**
 * Same state? Start and end with a few milliseconds of slack: the length the
 * video element reports varies by fractions between two loads — without slack
 * every freshly opened clip would count as changed.
 */
function sameEdit(a: ClipEdit, b: ClipEdit): boolean {
  const near = (x: number, y: number) => Math.abs(x - y) <= 50;
  return (
    near(a.startMs, b.startMs) &&
    near(a.endMs, b.endMs) &&
    a.tracks.length === b.tracks.length &&
    a.tracks.every((track, i) => {
      const other = b.tracks[i];
      return (
        track.index === other.index &&
        track.gainDb === other.gainDb &&
        track.muted === other.muted
      );
    })
  );
}

function Scrubber({
  progress,
  fillRef,
  knobRef,
  duration,
  trim,
  waveform,
  onSeek,
  onTrim,
}: {
  progress: number;
  /** Bar and handle. While playing, the player's frame loop writes straight
      into them instead of triggering a render. */
  fillRef: React.RefObject<HTMLDivElement | null>;
  knobRef: React.RefObject<HTMLDivElement | null>;
  duration: number;
  trim: Trim;
  /** Picture of the audio track; missing while ffmpeg is still drawing it. */
  waveform?: string;
  onSeek: (ratio: number) => void;
  onTrim: (trim: Trim) => void;
}) {
  const bar = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState<"start" | "end" | null>(null);
  const percent = (seconds: number) =>
    duration > 0 ? Math.min(100, Math.max(0, (seconds / duration) * 100)) : 0;

  /** Drag one of the two handles. */
  const drag = (which: "start" | "end") => (event: React.PointerEvent) => {
    event.preventDefault();
    event.stopPropagation();
    if (duration <= 0) return;
    const box = bar.current?.getBoundingClientRect();
    if (!box) return;
    setDragging(which);

    const move = (moved: PointerEvent) => {
      const ratio = Math.min(
        Math.max((moved.clientX - box.left) / box.width, 0),
        1,
      );
      const at = ratio * duration;
      onTrim(
        which === "start"
          ? { start: Math.min(at, trim.end - 0.5), end: trim.end }
          : { start: trim.start, end: Math.max(at, trim.start + 0.5) },
      );
    };
    const up = () => {
      setDragging(null);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return (
    <div
      ref={bar}
      role="slider"
      aria-label="Position"
      aria-valuenow={Math.round(progress)}
      aria-valuemin={0}
      aria-valuemax={100}
      tabIndex={0}
      onClick={(event) => {
        const box = event.currentTarget.getBoundingClientRect();
        onSeek(Math.min(Math.max((event.clientX - box.left) / box.width, 0), 1));
      }}
      className={cn(
        "group relative h-9 flex-1 cursor-pointer",
        duration === 0 && "pointer-events-none opacity-40",
      )}
    >
      {/* The audio track as a picture: you can see where something happens
          before listening for it. */}
      {waveform && (
        <div
          className="pointer-events-none absolute inset-x-0 inset-y-1 rounded-[3px] opacity-40"
          style={{
            backgroundImage: `url(${waveform})`,
            backgroundSize: "100% 100%",
            backgroundRepeat: "no-repeat",
          }}
        />
      )}

      <div className="absolute inset-x-0 top-1/2 h-1 -translate-y-1/2 rounded-pill bg-white/15">
        {/* No width transition: it would work against the frame loop and make
            the motion stutter again. */}
        <div
          ref={fillRef}
          className="h-full rounded-pill bg-accent-bright"
          style={{ width: `${progress}%` }}
        />
      </div>

      {/* What falls away lies behind a veil. */}
      <div
        className="pointer-events-none absolute inset-y-0 left-0 rounded-l-pill bg-black/55"
        style={{ width: `${percent(trim.start)}%` }}
      />
      <div
        className="pointer-events-none absolute inset-y-0 right-0 rounded-r-pill bg-black/55"
        style={{ width: `${100 - percent(trim.end)}%` }}
      />
      <TrimHandle
        at={percent(trim.start)}
        label="Start"
        time={dragging === "start" ? clock(trim.start) : null}
        onDrag={drag("start")}
      />
      <TrimHandle
        at={percent(trim.end)}
        label="End"
        time={dragging === "end" ? clock(trim.end) : null}
        onDrag={drag("end")}
      />

      <div
        ref={knobRef}
        className="pointer-events-none absolute top-1/2 h-3 w-3 -translate-x-1/2 -translate-y-1/2
          rounded-pill bg-white opacity-0 transition-opacity group-hover:opacity-100"
        style={{ left: `${progress}%` }}
      />
    </div>
  );
}

function TrimHandle({
  at,
  label,
  time,
  onDrag,
}: {
  at: number;
  label: string;
  /** Show the position while dragging — otherwise you trim by feel. */
  time: string | null;
  onDrag: (event: React.PointerEvent) => void;
}) {
  return (
    <button
      aria-label={`${label} des Ausschnitts`}
      onPointerDown={onDrag}
      onClick={(event) => event.stopPropagation()}
      // The hit area is as tall as the bar; only the handle in the middle is
      // visible. Nobody hits a 5 pixel target twice.
      className="group/handle absolute inset-y-0 w-4 -translate-x-1/2 cursor-ew-resize"
      style={{ left: `${at}%` }}
    >
      <span
        className="absolute top-1/2 left-1/2 h-6 w-[3px] -translate-x-1/2 -translate-y-1/2
          rounded-pill bg-accent-bright shadow transition-[height] group-hover/handle:h-7"
      />
      {time && (
        <span
          className="absolute -top-6 left-1/2 -translate-x-1/2 rounded-pill bg-black/80 px-2
            py-0.5 font-mono text-[11px] text-ink tabular-nums"
        >
          {time}
        </span>
      )}
    </button>
  );
}

function Volume({
  value,
  onChange,
  onToggleMute,
}: {
  value: number;
  onChange: (value: number) => void;
  onToggleMute: () => void;
}) {
  return (
    <div className="flex shrink-0 items-center gap-2">
      <button
        aria-label={value === 0 ? "Unmute" : "Mute"}
        onClick={onToggleMute}
        className="grid h-9 w-9 place-items-center rounded-pill text-ink-muted
          transition-colors hover:bg-elevated hover:text-ink"
      >
        <svg viewBox="0 0 24 24" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinejoin="round">
          <path d="M4 9.5h3.5L12 5.5v13L7.5 14.5H4v-5Z" />
          {value === 0 ? (
            <path d="M16 10l4 4M20 10l-4 4" strokeLinecap="round" />
          ) : (
            <path d="M15.5 9.5a4 4 0 0 1 0 5" strokeLinecap="round" />
          )}
        </svg>
      </button>
      <input
        type="range"
        aria-label="Volume"
        min={0}
        max={1}
        step={0.01}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        className="h-1 w-20 cursor-pointer appearance-none rounded-pill bg-white/15
          [&::-webkit-slider-thumb]:h-3 [&::-webkit-slider-thumb]:w-3
          [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:rounded-pill
          [&::-webkit-slider-thumb]:bg-white"
      />
    </div>
  );
}

function Step({
  label,
  disabled,
  onClick,
  children,
}: {
  label: string;
  disabled: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      aria-label={label}
      disabled={disabled}
      onClick={onClick}
      className="grid h-8 w-8 place-items-center rounded-pill border border-line text-lg
        text-ink-muted transition-colors hover:bg-elevated hover:text-ink
        disabled:pointer-events-none disabled:opacity-30"
    >
      {children}
    </button>
  );
}

/** Like `formatDuration`, but for running times (seconds instead of milliseconds). */
function clock(seconds: number): string {
  if (!Number.isFinite(seconds)) return "0:00";
  const total = Math.floor(seconds);
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}
