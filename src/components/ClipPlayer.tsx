import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/Button";
import { Pill } from "@/components/ui/Card";
import { IconTrash } from "@/components/icons";
import { api, fileUrl, inTauri } from "@/lib/ipc";
import { formatAgo, formatSize } from "@/lib/format";
import { cn } from "@/lib/cn";
import type { Clip } from "@/lib/types";

interface Props {
  clips: Clip[];
  index: number;
  onIndexChange: (index: number) => void;
  onClose: () => void;
  onDelete: (id: string) => void;
}

/**
 * Vollflächiger Player über der Galerie.
 *
 * Hinweis zu mehreren Tonspuren: WebView2 stellt `HTMLMediaElement.audioTracks`
 * nicht bereit, hier läuft deshalb immer der Hauptmix. Die zusätzlichen Spuren
 * bleiben in der Datei und stehen im Schnittprogramm zur Verfügung.
 */
export function ClipPlayer({
  clips,
  index,
  onIndexChange,
  onClose,
  onDelete,
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
  }, [clip?.id]);

  useEffect(() => {
    const element = video.current;
    if (element) element.volume = muted ? 0 : volume;
  }, [volume, muted, clip?.id]);

  // `timeupdate` feuert nur etwa viermal pro Sekunde — die Leiste würde
  // sichtbar springen. Solange abgespielt wird, liest ein Frame-Loop die
  // Position direkt aus dem Element.
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    const tick = () => {
      const element = video.current;
      if (element) setTime(element.currentTime);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing]);

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

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      // Im Suchfeld darf die Leertaste weiterhin ein Leerzeichen sein.
      if (event.target instanceof HTMLInputElement) return;
      const handlers: Record<string, () => void> = {
        " ": toggle,
        k: toggle,
        ArrowRight: () => seek(5),
        ArrowLeft: () => seek(-5),
        ArrowUp: () => setVolume((v) => Math.min(1, v + 0.1)),
        ArrowDown: () => setVolume((v) => Math.max(0, v - 0.1)),
        m: () => setMuted((m) => !m),
        f: fullscreen,
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
  }, [toggle, seek, fullscreen, step, onClose]);

  if (!clip) return null;

  const source = fileUrl(clip.path);
  const name = clip.path.split("\\").pop() ?? clip.path;
  const progress = duration > 0 ? (time / duration) * 100 : 0;

  return (
    <div
      className="fixed inset-0 z-50 flex flex-col bg-black/80 backdrop-blur-xl"
      onClick={onClose}
    >
      <header className="flex shrink-0 items-start justify-between gap-6 px-8 pt-6 pb-4">
        <div className="min-w-0" onClick={(e) => e.stopPropagation()}>
          <div className="flex items-center gap-2">
            <h2 className="display truncate text-2xl">
              {clip.game ?? "Unbekanntes Spiel"}
            </h2>
            <Pill className="bg-white/10">{clip.height}p</Pill>
          </div>
          <p className="mt-1 truncate text-xs text-ink-muted">
            {name} · {formatAgo(clip.createdAt)} · {formatSize(clip.sizeBytes)}
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
        ref={frame}
        className="relative mx-8 min-h-0 flex-1 overflow-hidden rounded-card bg-black"
        onClick={(e) => e.stopPropagation()}
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
            onPause={() => setPlaying(false)}
            onTimeUpdate={(e) => {
              // Nur noch für den pausierten Zustand und fürs Spulen relevant.
              if (e.currentTarget.paused) setTime(e.currentTarget.currentTime);
            }}
            onSeeked={(e) => setTime(e.currentTarget.currentTime)}
            onLoadedMetadata={(e) => {
              setDuration(e.currentTarget.duration);
              e.currentTarget.volume = muted ? 0 : volume;
            }}
            onEnded={() => setPlaying(false)}
            onError={() => setBroken(true)}
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
            duration={duration}
            onSeek={(ratio) => {
              const element = video.current;
              if (element && Number.isFinite(element.duration)) {
                element.currentTime = ratio * element.duration;
              }
            }}
          />

          <span className="shrink-0 font-mono text-xs text-ink-muted tabular-nums">
            {clock(time)} / {clock(duration)}
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
            Leertaste · ←/→ 5 s · M stumm · F Vollbild
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
  duration,
  onSeek,
}: {
  progress: number;
  duration: number;
  onSeek: (ratio: number) => void;
}) {
  return (
    <div
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
          className="h-full rounded-pill bg-accent-bright"
          style={{ width: `${progress}%` }}
        />
      </div>
      <div
        className="absolute top-1/2 h-3 w-3 -translate-x-1/2 -translate-y-1/2 rounded-pill bg-white
          opacity-0 transition-opacity group-hover:opacity-100"
        style={{ left: `${progress}%` }}
      />
    </div>
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
