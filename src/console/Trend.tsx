import { useCallback, useRef, useState } from "react";
import type { EngineStatus } from "@/lib/types";

/**
 * Der Verlauf der letzten Minute — Vorschlag k07 aus dem Konsolen-Labor.
 *
 * Ein einzelner Einbruch ist vorbei, bevor man hinsieht: die Kachel „Bilder/s"
 * steht auf 60, und dass sie vor vierzig Sekunden auf 22 stand, weiß niemand
 * mehr. Die Konsole schreibt die Werte deshalb selbst mit.
 *
 * Kein Backend nötig: `engine-status` geht im Sekundentakt an **jedes** Fenster
 * (`lib.rs:867`), auch an das versteckte. Die Konsole sammelt also weiter,
 * während sie zu ist — und genau das ist der Gewinn. Man macht sie ja auf,
 * *weil* gerade etwas geruckelt hat, und findet den Einbruch dann noch vor.
 */

/** Wie weit der Verlauf zurückreicht. Ein Eintrag ist eine Sekunde, weil der
 *  Status im Sekundentakt kommt. */
export const TREND_SECONDS = 60;

export interface Sample {
  /** Die gemessene Bildrate dieser Sekunde. */
  fps: number;
  /** Verworfene Bilder **in dieser Sekunde** — nicht der Zählerstand.
   *  `EngineStatus.droppedFrames` zählt seit dem Start der Pipeline hoch. */
  dropped: number;
  /** Lief der Puffer überhaupt? Ohne ihn ist die Bildrate 0, und das ist kein
   *  Einbruch, sondern eine Pause — die Kurve bricht dort ab, statt auf den
   *  Boden zu fallen. */
  active: boolean;
}

export interface Trend {
  samples: Sample[];
  /** Was in der letzten Minute verworfen wurde. */
  dropped: number;
  /** Sekunden seit dem letzten Einbruch, `null` wenn es keinen gab. */
  dipAgo: number | null;
}

/**
 * Sammelt den Verlauf. `push` gehört in den `engine-status`-Zuhörer, direkt
 * neben `setStatus` — beide Änderungen liegen dann im selben Rendern.
 */
export function useTrend(targetFps: number): Trend & { push: (s: EngineStatus) => void } {
  const [samples, setSamples] = useState<Sample[]>([]);
  /** Der Zählerstand aus dem vorigen Status, um den Zuwachs zu bilden.
   *
   *  Fortgeschrieben wird er im Ereignis selbst und nicht im Aktualisierer von
   *  `setSamples`: ein Aktualisierer muss rein sein, StrictMode ruft ihn
   *  zweimal auf, und der zweite Aufruf hätte die Sekunde ein zweites Mal
   *  verrechnet. Der Zuhörer läuft einmal, also steht der Zuwachs hier. */
  const seen = useRef<number | null>(null);

  const push = useCallback((status: EngineStatus) => {
    const before = seen.current;
    seen.current = status.droppedFrames;
    // Ein Neustart der Pipeline setzt den Zähler zurück; der Unterschied wäre
    // dann negativ. Dann beginnt die Zählung eben neu.
    const grew =
      before === null || status.droppedFrames < before
        ? 0
        : status.droppedFrames - before;
    setSamples((prev) =>
      prev
        .concat({ fps: status.fps, dropped: grew, active: status.bufferActive })
        .slice(-TREND_SECONDS),
    );
  }, []);

  const dropped = samples.reduce((sum, s) => sum + s.dropped, 0);

  // Ein Einbruch ist eine Sekunde, in der der Puffer lief und trotzdem deutlich
  // weniger Bilder ankamen als eingestellt. Die Schwelle ist großzügig: bei 60
  // eingestellten schwankt die gemessene Rate um ein, zwei Bilder, und jedes
  // davon als Einbruch zu melden hieße, dauernd zu melden.
  const floor = targetFps * 0.8;
  let dipAgo: number | null = null;
  for (let i = samples.length - 1; i >= 0; i--) {
    const s = samples[i];
    if (s.active && s.fps < floor) {
      dipAgo = samples.length - 1 - i;
      break;
    }
  }

  return { samples, dropped, dipAgo, push };
}

/** Die Kurve selbst. Ohne Achsen und ohne Zahlen — die stehen in den Kacheln
 *  darunter; hier geht es allein um die Form. */
export function TrendChart({
  samples,
  targetFps,
}: {
  samples: Sample[];
  targetFps: number;
}) {
  // Das Koordinatensystem ist erfunden und wird über `preserveAspectRatio` auf
  // die Breite des Panels gezogen. Die Striche bleiben trotzdem gleich dick:
  // `vector-effect` nimmt sie von der Streckung aus.
  const W = 300;
  const H = 40;
  /** Oben die eingestellte Rate, unten die Null. Nicht ganz an den Rand: eine
   *  Linie auf der Kante sieht aus wie abgeschnitten. */
  const top = 6;
  const floor = 32;
  const step = W / (TREND_SECONDS - 1);
  /** Die neueste Sekunde steht rechts. Ist noch nicht die volle Minute
   *  zusammengekommen, läuft die Kurve von rechts herein, statt links zu
   *  beginnen und etwas zu behaupten, das nicht gemessen wurde. */
  const xOf = (i: number) => W - (samples.length - 1 - i) * step;
  const yOf = (fps: number) =>
    floor - Math.min(1, Math.max(0, fps / Math.max(1, targetFps))) * (floor - top);

  // Jede Strecke, in der der Puffer durchgehend lief, ist ein eigener Pfad —
  // sonst zöge die Kurve quer durch eine Pause, als wäre dort gemessen worden.
  const runs: string[] = [];
  let run: string[] = [];
  for (let i = 0; i < samples.length; i++) {
    const s = samples[i];
    if (!s.active) {
      if (run.length > 1) runs.push(run.join(" "));
      run = [];
      continue;
    }
    run.push(`${run.length === 0 ? "M" : "L"}${xOf(i).toFixed(1)} ${yOf(s.fps).toFixed(1)}`);
  }
  if (run.length > 1) runs.push(run.join(" "));

  return (
    <svg
      viewBox={`0 0 ${W} ${H}`}
      preserveAspectRatio="none"
      role="img"
      aria-label={`Bildrate der letzten ${TREND_SECONDS} Sekunden`}
      className="cb-trend"
    >
      {/* Wo die Kurve liegen sollte: die eingestellte Rate als Marke, damit ein
          Einbruch nicht geraten, sondern gesehen wird. */}
      <line
        x1="0"
        y1={top}
        x2={W}
        y2={top}
        className="cb-trend-target"
        vectorEffect="non-scaling-stroke"
      />
      {runs.map((d) => (
        <path
          key={d}
          d={d}
          fill="none"
          className="cb-trend-line"
          vectorEffect="non-scaling-stroke"
        />
      ))}
      {/* Verworfene Bilder als Zacken vom Boden her. Sie liegen unter der
          Kurve, weil sie meistens zusammen auftreten — und nur dort, wo
          wirklich etwas fehlt. */}
      {samples.map((s, i) =>
        s.dropped > 0 ? (
          <line
            key={i}
            x1={xOf(i)}
            y1={H}
            x2={xOf(i)}
            y2={H - 4 - Math.min(1, s.dropped / 5) * 10}
            className="cb-trend-drop"
            vectorEffect="non-scaling-stroke"
          />
        ) : null,
      )}
    </svg>
  );
}
