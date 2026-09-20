import { useRef, useState } from "react";
import { cn } from "@/lib/cn";
import type { Trim } from "@/components/ClipEditor";

/**
 * Die Bedienleiste unter einem Video.
 *
 * Sie steht hier und nicht im Player, weil es zwei Player gibt: den in der App
 * (`ClipPlayer`) und den in der Konsole über dem Spiel (`console/Console.tsx`).
 * Der Unterschied zwischen beiden ist der Ausschnitt — über dem Spiel wird nicht
 * geschnitten —, deshalb sind `trim`, `waveform` und `onTrim` freiwillig.
 */

/** Wie `formatDuration`, nur für Laufzeiten (Sekunden statt Millisekunden). */
export function clock(seconds: number): string {
  if (!Number.isFinite(seconds)) return "0:00";
  const total = Math.floor(seconds);
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

/** Springt, und bleibt dabei innerhalb der Datei. */
export function jump(element: HTMLVideoElement, seconds: number) {
  element.currentTime = Math.min(Math.max(seconds, 0), element.duration);
}

/**
 * Eines der beiden Zeichen auf dem Play-Knopf. Das gehende schrumpft und dreht
 * sich weg, das kommende dreht sich von der anderen Seite herein — 160 ms, also
 * überlappen sie sich und es liest sich als ein Zeichen, das es sich anders
 * überlegt.
 */
export function PlayGlyph({
  shown,
  turn = 1,
  children,
}: {
  shown: boolean;
  turn?: 1 | -1;
  children: React.ReactNode;
}) {
  return (
    <span
      aria-hidden={!shown}
      className={cn(
        "absolute grid place-items-center",
        "transition-[opacity,transform,rotate] duration-[160ms] ease-[var(--ease-out-soft)]",
        shown
          ? "scale-100 opacity-100"
          : "pointer-events-none scale-[0.7] opacity-0",
      )}
      style={{ rotate: shown ? "0deg" : `${20 * turn}deg` }}
    >
      {children}
    </span>
  );
}

/** Der weiße runde Knopf, der zwischen Play und Pause wechselt. */
export function PlayButton({
  playing,
  disabled,
  onClick,
  size = "lg",
}: {
  playing: boolean;
  disabled?: boolean;
  onClick: () => void;
  /** `sm` ist der Knopf in der Konsole, wo die Leiste in der Karte sitzt. Ein
      `className` wäre hier falsch: `cn` hängt nur aneinander, zwei Tailwind-
      Größen nebeneinander entscheidet dann die Reihenfolge im Stylesheet. */
  size?: "lg" | "sm";
}) {
  return (
    <button
      aria-label={playing ? "Pause" : "Play"}
      onClick={onClick}
      disabled={disabled}
      className={cn(
        `relative grid shrink-0 place-items-center rounded-pill bg-white
         text-black transition-transform active:scale-95 disabled:opacity-40`,
        size === "lg" ? "h-11 w-11" : "h-9 w-9",
      )}
    >
      {/* Beide Zeichen bleiben stehen und übergeben aneinander. Die Elemente zu
          tauschen ließe den Knopf genau dann blinken, wenn das Auge auf ihm
          liegt. */}
      <PlayGlyph shown={playing}>
        <svg viewBox="0 0 24 24" className="h-4 w-4" fill="currentColor">
          <rect x="6.5" y="5" width="3.6" height="14" rx="1.2" />
          <rect x="13.9" y="5" width="3.6" height="14" rx="1.2" />
        </svg>
      </PlayGlyph>
      <PlayGlyph shown={!playing} turn={-1}>
        <svg viewBox="0 0 24 24" className="h-4 w-4 translate-x-[1px]" fill="currentColor">
          <path d="M7.5 5.2 19 12 7.5 18.8V5.2Z" />
        </svg>
      </PlayGlyph>
    </button>
  );
}

export function Scrubber({
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
  /** Balken und Griff. Während des Abspielens schreibt die Bildschleife des
      Players direkt hinein, statt ein Rendern auszulösen. */
  fillRef: React.RefObject<HTMLDivElement | null>;
  knobRef: React.RefObject<HTMLDivElement | null>;
  duration: number;
  /** Ohne Ausschnitt bleiben Schleier und Griffe weg — so benutzt die Konsole
      dieselbe Leiste. */
  trim?: Trim;
  /** Bild der Tonspur; fehlt, solange ffmpeg noch zeichnet. */
  waveform?: string;
  onSeek: (ratio: number) => void;
  onTrim?: (trim: Trim) => void;
}) {
  const bar = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState<"start" | "end" | null>(null);
  const percent = (seconds: number) =>
    duration > 0 ? Math.min(100, Math.max(0, (seconds / duration) * 100)) : 0;

  /** Einen der beiden Griffe ziehen. */
  const drag = (which: "start" | "end") => (event: React.PointerEvent) => {
    event.preventDefault();
    event.stopPropagation();
    if (duration <= 0 || !trim || !onTrim) return;
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
      {/* Die Tonspur als Bild: man sieht, wo etwas passiert, bevor man danach
          hört. */}
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
        {/* Kein Übergang auf der Breite: er arbeitete gegen die Bildschleife und
            ließ die Bewegung wieder stocken. */}
        <div
          ref={fillRef}
          className="h-full rounded-pill bg-accent-bright"
          style={{ width: `${progress}%` }}
        />
      </div>

      {trim && onTrim && (
        <>
          {/* Was wegfällt, liegt hinter einem Schleier. */}
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
        </>
      )}

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
  /** Die Stelle beim Ziehen zeigen — sonst schneidet man nach Gefühl. */
  time: string | null;
  onDrag: (event: React.PointerEvent) => void;
}) {
  return (
    <button
      aria-label={`${label} des Ausschnitts`}
      onPointerDown={onDrag}
      onClick={(event) => event.stopPropagation()}
      // Die Trefferfläche ist so hoch wie der Balken; sichtbar ist nur der Griff
      // in der Mitte. Ein 5-Pixel-Ziel trifft niemand zweimal.
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

export function Volume({
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
