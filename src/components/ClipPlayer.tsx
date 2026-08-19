import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/Button";
import { Pill } from "@/components/ui/Card";
import { ClipEditor, type Trim } from "@/components/ClipEditor";
import { IconTrash } from "@/components/icons";
import { api, fileUrl, inTauri } from "@/lib/ipc";
import { clipName, formatAgo, formatSize } from "@/lib/format";
import { useClipMix } from "@/lib/useClipMix";
import { cn } from "@/lib/cn";
import type { Clip } from "@/lib/types";

interface Props {
  clips: Clip[];
  index: number;
  onIndexChange: (index: number) => void;
  onClose: () => void;
  onDelete: (id: string) => void;
  /** Mit offenem Bearbeiten-Bereich starten. */
  startEditing?: boolean;
}

/**
 * Vollflächiger Player über der Galerie, mit angehängtem Bearbeiten-Bereich.
 *
 * Hinweis zu mehreren Tonspuren: WebView2 stellt `HTMLMediaElement.audioTracks`
 * nicht bereit, das Videoelement gibt deshalb immer nur den Hauptmix wieder.
 * Die übrigen Spuren entpackt der Kern einzeln und `useClipMix` lässt sie
 * synchron mitlaufen — nur so lässt sich eine Mischung überhaupt beurteilen.
 */
export function ClipPlayer({
  clips,
  index,
  onIndexChange,
  onClose,
  onDelete,
  startEditing = false,
}: Props) {
  const clip = clips[index];
  const video = useRef<HTMLVideoElement>(null);
  const frame = useRef<HTMLDivElement>(null);

  const [playing, setPlaying] = useState(false);
  const [time, setTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [volume, setVolume] = useState(1);
  const [muted, setMuted] = useState(false);
  const [broken, setBroken] = useState(false);
  const [editing, setEditing] = useState(startEditing);
  const [trim, setTrim] = useState<Trim>({ start: 0, end: 0 });

  // Die Lautstärke des Videoelements gehört dem Mixer: Spur 0 ist der
  // Hauptmix, und der muss zu den übrigen Spuren passen.
  const mix = useClipMix(clip, video, muted ? 0 : volume);

  const step = useCallback(
    (delta: number) => {
      const next = index + delta;
      if (next >= 0 && next < clips.length) onIndexChange(next);
    },
    [clips.length, index, onIndexChange],
  );

  // Beim Clipwechsel alles zurücksetzen.
  useEffect(() => {
    setBroken(false);
    setTime(0);
    setDuration(0);
    setTrim({ start: 0, end: 0 });
  }, [clip?.id]);

  // Die Position wird beim Abspielen ohne React gezeichnet: Der Frame-Loop
  // schreibt Balken, Griff und Uhr direkt ins DOM.
  //
  // Vorher stand hier ein `setTime` pro Bild, also sechzig Renders in der
  // Sekunde. Der WebView2 rendert React und das Video auf demselben Faden und
  // ließ darüber Videobilder fallen — der Clip sah aus, als ruckelte er und
  // liefe dem Ton davon. Nachgemessen ist die Datei dabei tadellos: 1973 von
  // 1975 Bildabständen exakt 17 ms, Bild und Ton 18 ms auseinander.
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

  // `timeupdate` feuert nur etwa viermal pro Sekunde — die Leiste würde
  // sichtbar springen. Solange abgespielt wird, liest ein Frame-Loop die
  // Position direkt aus dem Element. Der Zuschnitt wird hier mit durchgesetzt.
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    const tick = () => {
      const element = video.current;
      if (element) {
        // Am gesetzten Ende zurück an den Anfang der Auswahl — beim Zuschneiden
        // will man die Stelle mehrfach hören, nicht den Rest des Clips.
        if (trim.end > 0 && element.currentTime >= trim.end) {
          element.currentTime = trim.start;
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
    if (element.paused) void element.play();
    else element.pause();
  }, []);

  const seek = useCallback((seconds: number) => {
    const element = video.current;
    if (!element || !Number.isFinite(element.duration)) return;
    element.currentTime = Math.min(
      Math.max(element.currentTime + seconds, 0),
      element.duration,
    );
  }, []);

  const fullscreen = useCallback(() => {
    if (document.fullscreenElement) void document.exitFullscreen();
    else void frame.current?.requestFullscreen();
  }, []);

  /** Anfang oder Ende der Auswahl auf die aktuelle Stelle legen. */
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
      // In Textfeldern bleiben Leertaste und Buchstaben, was sie sind.
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
        ArrowRight: () => seek(5),
        ArrowLeft: () => seek(-5),
        ArrowUp: () => setVolume((v) => Math.min(1, v + 0.1)),
        ArrowDown: () => setVolume((v) => Math.max(0, v - 0.1)),
        m: () => setMuted((m) => !m),
        f: fullscreen,
        e: () => setEditing((current) => !current),
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
  }, [toggle, seek, fullscreen, mark, step, onClose]);

  if (!clip) return null;

  const source = fileUrl(clip.path);
  const name = clipName(clip);
  const progress = duration > 0 ? (time / duration) * 100 : 0;

  return (
    <div
      className="fixed inset-0 z-50 flex flex-col bg-black/80 backdrop-blur-xl"
      onClick={onClose}
    >
      <header className="flex shrink-0 items-start justify-between gap-6 px-8 pt-6 pb-4">
        <div className="min-w-0" onClick={(e) => e.stopPropagation()}>
          <div className="flex items-center gap-2">
            <h2 className="display truncate text-2xl">{name}</h2>
            <Pill className="bg-white/10">{clip.height}p</Pill>
          </div>
          <p className="mt-1 truncate text-xs text-ink-muted">
            {clip.game ?? "Unbekanntes Spiel"} · {formatAgo(clip.createdAt)} ·{" "}
            {formatSize(clip.sizeBytes)}
          </p>
        </div>
        <button
          aria-label="Player schließen"
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
          className="relative min-h-0 min-w-0 flex-1 overflow-hidden rounded-card bg-black"
        >
          {broken || !source ? (
            <div className="grid h-full place-items-center px-8 text-center">
              <div>
                <p className="text-sm font-medium">Datei nicht gefunden</p>
                <p className="mx-auto mt-2 max-w-md text-xs text-ink-muted">
                  {inTauri
                    ? `Die Datei unter ${clip.path} lässt sich nicht öffnen — vermutlich wurde sie außerhalb von ClippiBoy verschoben oder gelöscht.`
                    : "Im Browser-Modus gibt es keine echten Clips."}
                </p>
              </div>
            </div>
          ) : (
            <video
              ref={video}
              src={source}
              autoPlay
              className="h-full w-full bg-black object-contain"
              onClick={toggle}
              onDoubleClick={fullscreen}
              onPlay={() => setPlaying(true)}
              // Beim Anhalten übernimmt React die Position wieder — sonst
              // spränge sie beim nächsten Render auf den Stand von vor dem
              // Abspielen zurück.
              onPause={(e) => {
                setPlaying(false);
                setTime(e.currentTarget.currentTime);
              }}
              onTimeUpdate={(e) => {
                // Nur noch für den pausierten Zustand und fürs Spulen relevant.
                if (e.currentTarget.paused) setTime(e.currentTarget.currentTime);
              }}
              onSeeked={(e) => setTime(e.currentTarget.currentTime)}
              onLoadedMetadata={(e) => {
                const length = e.currentTarget.duration;
                setDuration(length);
                // Ohne eigenen Zuschnitt ist der ganze Clip ausgewählt.
                if (Number.isFinite(length)) setTrim({ start: 0, end: length });
              }}
              onEnded={(e) => {
                setPlaying(false);
                setTime(e.currentTarget.currentTime);
              }}
              onError={() => setBroken(true)}
            />
          )}
        </div>

        {editing && (
          <ClipEditor
            clip={clip}
            duration={duration}
            tracks={mix.tracks}
            mix={mix.mix}
            loadingTracks={mix.loading}
            onTrack={mix.setTrack}
            onResetMix={mix.reset}
            trim={trim}
            onTrim={setTrim}
            onMark={mark}
            toRequest={mix.toRequest}
          />
        )}
      </div>

      <footer
        className="shrink-0 space-y-3 px-8 pt-4 pb-6"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-4">
          <button
            aria-label={playing ? "Pause" : "Abspielen"}
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
            trim={editing ? trim : null}
            onSeek={(ratio) => {
              const element = video.current;
              if (element && Number.isFinite(element.duration)) {
                element.currentTime = ratio * element.duration;
              }
            }}
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
            aria-label="Vollbild"
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
          <Button
            size="sm"
            variant={editing ? "primary" : "secondary"}
            onClick={() => setEditing((current) => !current)}
          >
            Bearbeiten
          </Button>
          <Button
            size="sm"
            variant="secondary"
            onClick={() => inTauri && api.revealClip(clip.id)}
          >
            Im Ordner zeigen
          </Button>
          <Button
            size="sm"
            variant="danger"
            icon={<IconTrash className="h-4 w-4" />}
            onClick={() => {
              onDelete(clip.id);
              if (clips.length <= 1) onClose();
              else onIndexChange(Math.min(index, clips.length - 2));
            }}
          >
            Löschen
          </Button>

          <span className="ml-auto text-xs text-ink-faint">
            {editing
              ? "I/O Anfang & Ende · E Bereich zu · Leertaste"
              : "Leertaste · ←/→ 5 s · M stumm · F Vollbild · E bearbeiten"}
          </span>

          <div className="flex items-center gap-1">
            <Step label="Vorheriger Clip" disabled={index === 0} onClick={() => step(-1)}>
              ‹
            </Step>
            <span className="w-16 text-center text-xs text-ink-muted tabular-nums">
              {index + 1} / {clips.length}
            </span>
            <Step
              label="Nächster Clip"
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

function Scrubber({
  progress,
  fillRef,
  knobRef,
  duration,
  trim,
  onSeek,
  onTrim,
}: {
  progress: number;
  /** Balken und Griff. Der Frame-Loop des Players schreibt beim Abspielen
      direkt hinein, statt einen Render auszulösen. */
  fillRef: React.RefObject<HTMLDivElement | null>;
  knobRef: React.RefObject<HTMLDivElement | null>;
  duration: number;
  /** `null`, solange nicht zugeschnitten wird. */
  trim: Trim | null;
  onSeek: (ratio: number) => void;
  onTrim: (trim: Trim) => void;
}) {
  const bar = useRef<HTMLDivElement>(null);
  const percent = (seconds: number) =>
    duration > 0 ? Math.min(100, Math.max(0, (seconds / duration) * 100)) : 0;

  /** Einen der beiden Griffe ziehen. */
  const drag = (which: "start" | "end") => (event: React.PointerEvent) => {
    event.preventDefault();
    event.stopPropagation();
    if (!trim || duration <= 0) return;
    const box = bar.current?.getBoundingClientRect();
    if (!box) return;

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
      <div className="absolute inset-x-0 top-1/2 h-1 -translate-y-1/2 rounded-pill bg-white/15">
        {/* Kein width-Übergang: er würde gegen den Frame-Loop arbeiten und
            die Bewegung wieder stockend machen. */}
        <div
          ref={fillRef}
          className="h-full rounded-pill bg-accent-bright"
          style={{ width: `${progress}%` }}
        />
      </div>

      {trim && (
        <>
          {/* Was wegfällt, liegt hinter einem Schleier. */}
          <div
            className="absolute inset-y-0 left-0 rounded-l-pill bg-black/55"
            style={{ width: `${percent(trim.start)}%` }}
          />
          <div
            className="absolute inset-y-0 right-0 rounded-r-pill bg-black/55"
            style={{ width: `${100 - percent(trim.end)}%` }}
          />
          <TrimHandle at={percent(trim.start)} label="Anfang" onDrag={drag("start")} />
          <TrimHandle at={percent(trim.end)} label="Ende" onDrag={drag("end")} />
        </>
      )}

      <div
        ref={knobRef}
        className="absolute top-1/2 h-3 w-3 -translate-x-1/2 -translate-y-1/2 rounded-pill bg-white
          opacity-0 transition-opacity group-hover:opacity-100"
        style={{ left: `${progress}%` }}
      />
    </div>
  );
}

function TrimHandle({
  at,
  label,
  onDrag,
}: {
  at: number;
  label: string;
  onDrag: (event: React.PointerEvent) => void;
}) {
  return (
    <button
      aria-label={`${label} des Ausschnitts`}
      onPointerDown={onDrag}
      onClick={(event) => event.stopPropagation()}
      className="absolute top-1/2 h-5 w-2.5 -translate-x-1/2 -translate-y-1/2 cursor-ew-resize
        rounded-[4px] border border-black/40 bg-accent-bright shadow"
      style={{ left: `${at}%` }}
    />
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
        aria-label={value === 0 ? "Ton an" : "Stumm"}
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
        aria-label="Lautstärke"
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

/** Wie `formatDuration`, aber für laufende Zeiten (Sekunden statt Millisekunden). */
function clock(seconds: number): string {
  if (!Number.isFinite(seconds)) return "0:00";
  const total = Math.floor(seconds);
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}
