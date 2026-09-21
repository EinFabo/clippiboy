import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Button } from "@/components/ui/Button";
import { Pill } from "@/components/ui/Card";
import { ClipEditor, type Trim } from "@/components/ClipEditor";
import { SaveVeil, type SaveVeilHandle } from "@/components/ui/BusyVeil";
import { ExportDialog } from "@/components/ExportDialog";
import { useClipMenu } from "@/components/clipMenu";
import { IconTrash } from "@/components/icons";
import { HeartBurst } from "@/components/ui/HeartBurst";
import { ConfirmDelete } from "@/components/ui/ConfirmDelete";
import {
  PlayButton,
  Scrubber,
  Volume,
  clock,
  jump,
} from "@/components/ui/PlayerControls";
import { EASE_ENTRANCE, EASE_EXIT, prefersReducedMotion } from "@/lib/motion";
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
  /**
   * Where the clip's tile sits on screen, if the caller shows tiles at all.
   * That is what lets the picture grow out of the gallery instead of the player
   * simply being there. The dashboard passes nothing and gets a plain fade.
   */
  originOf?: (clipId: string) => DOMRect | null;
  /**
   * Wo der Clip anfangen soll — die Konsole über dem Spiel reicht die Stelle
   * mit, an der dort gerade geschaut wurde. Gilt einmal, für den Clip, mit dem
   * der Player aufgeht.
   */
  startAt?: number;
}

/** The picture on its way between the tile and the stage. */
interface Flight {
  src: string;
  from: DOMRect;
  to: DOMRect;
}

/** One frame at 30 fps. Enough to set a mark cleanly. */
const FRAME = 1 / 30;

/** Has to match the length of `cb-player-out` in styles/motion.css. */
const LEAVE_MS = 200;

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
  originOf,
  startAt,
}: Props) {
  const clip = clips[index];
  const video = useRef<HTMLVideoElement>(null);
  /**
   * Closing is held for as long as the exit runs. The gallery moves files
   * around the moment the player is gone (Clips.tsx), so the wait has to happen
   * here rather than there — every caller gets it that way.
   */
  const [leaving, setLeaving] = useState(false);
  /** Set while the stage is travelling back onto its tile. */
  const [shrinking, setShrinking] = useState(false);
  const goodbye = useRef(0);
  const [flight, setFlight] = useState<Flight | null>(null);
  const ghost = useRef<HTMLImageElement>(null);
  const frame = useRef<HTMLDivElement>(null);
  const applyClipEdit = useEngine((state) => state.applyClipEdit);
  const setFavorite = useEngine((state) => state.setFavorite);
  const clipMenu = useClipMenu();
  const restoreClipOriginal = useEngine((state) => state.restoreClipOriginal);
  const discardClipOriginal = useEngine((state) => state.discardClipOriginal);

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
  /**
   * Whether the question about deleting stands. The file goes off the disk and
   * does not come back, so the button asks first — in its own place, like
   * everything else that asks in this app.
   */
  const [askingDelete, setAskingDelete] = useState(false);
  const [exporting, setExporting] = useState(false);
  // Counts the video element's restarts. Serves as a `key`: after saving, the
  // player gets a fresh element rather than one whose source we pulled out from
  // under it. The audio tracks re-attach to it as well.
  const [reload, setReload] = useState(0);
  /**
   * The write is through, but the fresh element has not reported a picture yet.
   * The veil stays up for that stretch — letting it go the moment the core is
   * finished would put a black frame between the still and the new video.
   */
  const [settling, setSettling] = useState(false);
  /** Where to jump back to after reloading. */
  const resumeAt = useRef<{ time: number; playing: boolean } | null>(null);
  /**
   * Die übergebene Startzeit, einmal zu verbrauchen. Ohne das Aufbrauchen
   * spränge auch der nächste Clip beim Weiterblättern auf dieselbe Sekunde.
   */
  const startOnce = useRef<number | null>(startAt ?? null);
  /** Failed attempts at loading the current file. */
  const loadFailures = useRef(0);
  /** Holds the last frame while the file underneath is being replaced. */
  const veil = useRef<SaveVeilHandle>(null);

  // A file that never loads must not leave the veil standing over the stage
  // for good. Nothing else can end `settling` if no picture ever arrives.
  useEffect(() => {
    if (!settling) return;
    const timer = window.setTimeout(() => setSettling(false), 4000);
    return () => clearTimeout(timer);
  }, [settling]);

  /** Play the exit, then really go. Everything that closes goes through here. */
  const close = useCallback(() => {
    if (prefersReducedMotion()) {
      onClose();
      return;
    }

    // The stage travels back onto the tile it came from. Not the thumbnail this
    // time but the stage itself, so what shrinks is the frame you were actually
    // watching — a thumbnail here would jump the clip back to its first second
    // at the very moment you close it.
    const stage = frame.current;
    const target = clip ? originOf?.(clip.id) : null;
    if (stage && target && target.width > 0) {
      const box = stage.getBoundingClientRect();
      stage.style.transformOrigin = "top left";
      stage.animate(
        [
          { transform: "none" },
          {
            transform:
              `translate(${target.left - box.left}px, ${target.top - box.top}px) ` +
              `scale(${target.width / box.width}, ${target.height / box.height})`,
          },
        ],
        { duration: LEAVE_MS, easing: EASE_EXIT, fill: "forwards" },
      );
      setShrinking(true);
    }

    setLeaving((already) => {
      if (already) return already;
      goodbye.current = window.setTimeout(onClose, LEAVE_MS);
      return true;
    });
  }, [clip, onClose, originOf]);

  useEffect(() => () => clearTimeout(goodbye.current), []);

  // Measured once, at the first layout: the tile is still where it was, and the
  // stage already knows how big it will be.
  useLayoutEffect(() => {
    const from = clip ? originOf?.(clip.id) : null;
    const to = frame.current?.getBoundingClientRect();
    if (!clip?.thumbPath || !from || !to || from.width === 0) return;
    if (prefersReducedMotion()) return;
    setFlight({ src: `${fileUrl(clip.thumbPath)}?v=${clip.sizeBytes}`, from, to });
    // Only for the clip the player opened with — paging on happens inside the
    // gallery's own list and has nothing to fly from.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const node = ghost.current;
    if (!flight || !node) return;

    const box = (rect: DOMRect) => ({
      left: `${rect.left}px`,
      top: `${rect.top}px`,
      width: `${rect.width}px`,
      height: `${rect.height}px`,
    });
    const fly = node.animate([box(flight.from), box(flight.to)], {
      duration: 350,
      easing: EASE_ENTRANCE,
      fill: "forwards",
    });
    // Hands over shortly before it lands, so the video is already underneath
    // rather than appearing after a gap.
    const hand = node.animate([{ opacity: 1 }, { opacity: 0 }], {
      delay: 250,
      duration: 150,
      fill: "forwards",
    });

    void Promise.all([fly.finished, hand.finished])
      .then(() => setFlight(null))
      .catch(() => {});

    return () => {
      fly.cancel();
      hand.cancel();
    };
  }, [flight]);

  // The video element's volume belongs to the mixer: it plays the clip's own
  // audio track, and that has to match the individual tracks.
  const mix = useClipMix(clip, video, muted ? 0 : volume, reload);

  // The success message belongs to exactly one clip — when paging through, it
  // would otherwise be a claim about a clip nobody saved. The same goes for the
  // question about deleting: it was asked about the clip that was on screen.
  useEffect(() => {
    setJustSaved(false);
    setAskingDelete(false);
  }, [clip?.id]);

  /** Delete and move on within the player — if the last clip is gone, there is
      nothing left to show. */
  const removeClip = useCallback(() => {
    if (!clip) return;
    setAskingDelete(false);
    onDelete(clip.id);
    if (clips.length <= 1) close();
    else onIndexChange(Math.min(index, clips.length - 2));
  }, [clip, clips.length, index, close, onDelete, onIndexChange]);

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
        // Escape works its way outwards: first the fullscreen, then the
        // question about deleting, and only then the player itself.
        Escape: () => {
          if (document.fullscreenElement) return;
          if (askingDelete) return; // the question takes it — see ConfirmDelete
          if (exporting) return; // likewise the export dialog
          close();
        },
      };
      const handler = handlers[event.key] ?? handlers[event.key.toLowerCase()];
      if (!handler) return;
      event.preventDefault();
      handler();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    toggle,
    seek,
    seekTo,
    fullscreen,
    mark,
    step,
    close,
    askingDelete,
    exporting,
    trim.start,
    trim.end,
  ]);

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
    // Take the picture first — a moment later there is nothing left to take.
    veil.current?.freeze(element);
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
      setSettling(true);
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
    veil.current?.freeze(element);
    element?.pause();
    element?.removeAttribute("src");
    element?.load();
    try {
      await restoreClipOriginal(clip.id);
    } catch {
      // The notice is already in the store.
    } finally {
      setRestoring(false);
      setSettling(true);
      setWriteProgress(null);
      setBroken(false);
      setReload((n) => n + 1);
    }
  }

  /**
   * Throw the untouched recording away. Nothing about the file being played
   * changes — only what lies beside it in the app data folder — so unlike
   * [save] and [restoreOriginal] the video element can stay where it is.
   */
  async function discardOriginal() {
    if (!clip || saving || restoring) return;
    try {
      await discardClipOriginal(clip.id);
    } catch {
      // The notice is already in the store.
    }
  }

  // A new state makes the success message obsolete.
  const showSaved = justSaved && !dirty;

  if (!clip) return null;

  const progress = duration > 0 ? (time / duration) * 100 : 0;

  // Hung under <body> rather than where it is written. The route wrapper it
  // would otherwise sit in animates a transform, and in Chromium that makes it
  // the frame of reference for every `fixed` descendant — the player would then
  // start below the nav bar instead of covering the window.
  return createPortal(
    <div
      className={cn(
        "fixed inset-0 z-50 bg-black/80 backdrop-blur-xl",
        leaving ? "cb-backdrop-out" : "cb-backdrop-in",
      )}
      onClick={close}
    >
      {/* The scale sits on this wrapper, not on the backdrop: a backdrop that
          scales drags the whole screen with it. */}
      <div
        className={cn(
          "flex h-full flex-col",
          leaving ? (shrinking ? "cb-chrome-out" : "cb-player-out") : "cb-player-in",
        )}
      >
      {/* The clip's name lives in the editing pane and is editable there — up
          here it would stand a second time, but untouchable.
          The bar is also what moves the window: the player covers the title
          bar, and without this there was nothing left to grab. The button
          stays out of it, like the ones in `TitleBar`. */}
      <header
        data-tauri-drag-region
        className="flex shrink-0 items-center justify-between gap-6 px-8 pt-6 pb-4"
      >
        <div
          data-tauri-drag-region
          className="flex min-w-0 items-center gap-2"
          onClick={(e) => e.stopPropagation()}
        >
          <p className="truncate text-xs text-ink-muted">
            {clip.game ?? "Unknown game"} · {formatAgo(clip.createdAt)} ·{" "}
            {formatSize(clip.sizeBytes)}
          </p>
          <Pill className="bg-white/10">{clip.height}p</Pill>
        </div>
        <button
          aria-label="Close player"
          onClick={close}
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
            clip &&
            clipMenu(event, clip, {
              onDelete: () => setAskingDelete(true),
              // No `onDiscardOriginal`: the editor pane beside the picture has
              // that button already, with its question in place.
              onExport: () => setExporting(true),
            })
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
                // There is a picture again — the still can go.
                setSettling(false);
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
                } else if (startOnce.current !== null) {
                  // Aus der Konsole über dem Spiel herübergereicht: dort stand
                  // der Clip an dieser Stelle, und hier steht er weiter.
                  const at = startOnce.current;
                  startOnce.current = null;
                  if (Number.isFinite(length)) {
                    element.currentTime = Math.min(Math.max(at, 0), length);
                  }
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
                setSettling(false);
              }}
            />
          )}

          {/* Sits over the picture, inside the frame that goes fullscreen, so
              it stays centred there too. The canvases underneath it are always
              mounted: `freeze` has to find them in the document before React
              has heard that anything is being saved at all. */}
          <SaveVeil
            ref={veil}
            active={saving || restoring || settling}
            progress={writeProgress}
            label={restoring ? "Restoring" : "Saving"}
          />
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
          onDiscard={() => void discardOriginal()}
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
          <PlayButton playing={playing} disabled={broken} onClick={toggle} />

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
              <HeartBurst
                favorite={clip.favorite}
                className={cn(!clip.favorite && "text-live")}
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
          {askingDelete ? (
            <ConfirmDelete
              question="Delete clip?"
              onConfirm={removeClip}
              onCancel={() => setAskingDelete(false)}
            />
          ) : (
            <Button
              size="sm"
              variant="danger"
              icon={<IconTrash className="h-4 w-4" />}
              onClick={() => setAskingDelete(true)}
            >
              Delete
            </Button>
          )}

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

      {flight && (
        <img
          ref={ghost}
          src={flight.src}
          alt=""
          aria-hidden
          className="pointer-events-none fixed z-[60] rounded-card bg-black object-contain"
          style={{
            left: flight.from.left,
            top: flight.from.top,
            width: flight.from.width,
            height: flight.from.height,
          }}
        />
      )}

      {exporting && clip && (
        <ExportDialog clip={clip} onClose={() => setExporting(false)} />
      )}
    </div>,
    document.body,
  );
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
