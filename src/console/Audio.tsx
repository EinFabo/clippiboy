import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "@/lib/ipc";
import { Slider } from "@/components/ui/Controls";
import { cn } from "@/lib/cn";
import type { AudioSource, LevelMap } from "@/lib/types";

/**
 * Das Ton-Panel der Konsole — Vorschlag k09 aus dem Konsolen-Labor.
 *
 * Der eine Griff, für den man sonst aus dem Spiel heraus muss: Discord ist zu
 * laut im Clip, das Mikrofon soll für die nächste Runde weg. Beides ist im
 * Mischer zwei Fenster und einen Alt-Tab entfernt, und nach dem Alt-Tab ist die
 * Runde vorbei.
 *
 * Gezeigt werden nur die eingeschalteten Quellen. Eine ausgeschaltete nimmt gar
 * nicht auf — die gehört in den Mischer, nicht über das Spiel.
 */

/** Die Balken, die gerade auf dem Schirm stehen, nach Quellen-Id.
 *
 *  Die Pegel kommen zwanzigmal die Sekunde. Ginge jedes Paket durch `useState`,
 *  baute sich zwanzigmal die Sekunde die ganze Konsole neu auf — über einem
 *  Spiel, auf demselben Faden, auf dem WebView2 auch zeichnet. Die Balken
 *  werden deshalb direkt in den DOM geschrieben, an React vorbei. Dasselbe
 *  Verfahren wie beim Abspielbalken in `ClipStage`, und aus demselben Grund. */
export type LevelBars = React.RefObject<Map<string, HTMLElement>>;

/**
 * Hört auf die Pegel und schreibt sie in die Balken.
 *
 * Gehört in die Konsole selbst und nicht ins Panel: der Zuhörer soll nicht bei
 * jedem Öffnen eines Panels ab- und wieder angemeldet werden. Steht kein Panel
 * offen, ist die Liste leer und das Paket kostet eine Schleife über nichts.
 */
export function useLevelBars(): LevelBars {
  const bars = useRef(new Map<string, HTMLElement>());
  useEffect(() => {
    if (!inTauri) return;
    const off = listen<LevelMap>("audio-levels", (e) => {
      for (const [id, bar] of bars.current) {
        // Stumm heißt stumm — der Kern misst weiter, was hereinkommt, aber im
        // Clip landet nichts davon. Ein zappelnder Balken an einer
        // stummgeschalteten Quelle behauptete das Gegenteil.
        const level = bar.dataset.muted === "true" ? 0 : (e.payload[id] ?? 0);
        bar.style.width = `${Math.min(1, Math.max(0, level)) * 100}%`;
      }
    });
    return () => {
      void off.then((fn) => fn());
    };
  }, []);
  return bars;
}

export function AudioPanel({
  sources,
  bars,
  onChange,
}: {
  sources: AudioSource[];
  bars: LevelBars;
  /** `now` heißt: sofort an den Kern, ohne die Bremse für den Regler. */
  onChange: (source: AudioSource, now?: boolean) => void;
}) {
  const shown = sources.filter((s) => s.enabled);
  if (shown.length === 0) {
    return (
      <p className="px-2 py-8 text-center text-sm text-ink-muted">
        Keine Tonquelle eingeschaltet — der Clip bliebe still.
      </p>
    );
  }
  return (
    <div className="grid gap-2.5">
      {shown.map((source) => (
        <Row key={source.id} source={source} bars={bars} onChange={onChange} />
      ))}
    </div>
  );
}

function Row({
  source,
  bars,
  onChange,
}: {
  source: AudioSource;
  bars: LevelBars;
  onChange: (source: AudioSource, now?: boolean) => void;
}) {
  const bar = useRef<HTMLDivElement>(null);

  // An- und abmelden mit der Zeile. Fällt das Panel zu, ist der Balken aus der
  // Liste und der Pegelstrom schreibt ins Nichts statt in einen toten Knoten.
  useEffect(() => {
    const element = bar.current;
    if (!element) return;
    const registry = bars.current;
    registry.set(source.id, element);
    return () => {
      registry.delete(source.id);
    };
  }, [bars, source.id]);

  return (
    <div className="flex items-center gap-3">
      <span className="w-36 shrink-0 truncate text-[13px] font-semibold">
        {source.label}
      </span>

      {/* Bei stumm bleibt der Balken auf null — die Bahn selbst färbt sich
          deshalb rot. Sonst sähe eine stummgeschaltete Quelle genauso aus wie
          eine, die gerade nur nichts zu sagen hat. */}
      <div
        className={cn(
          "cb-level h-1.5 min-w-0 flex-1 overflow-hidden rounded-pill",
          source.muted ? "bg-live/25" : "bg-black/35",
        )}
      >
        <div
          ref={bar}
          // Der Pegelstrom liest das hier, bevor er die Breite schreibt — so
          // steht der Stand immer an der Stelle, die ihn braucht, ohne dass
          // der Zuhörer die Quellen kennen müsste.
          data-muted={source.muted}
          className="cb-level-fill h-full rounded-pill"
          style={{ width: "0%" }}
        />
      </div>

      <div className="w-40 shrink-0">
        <Slider
          label={`Lautstärke ${source.label}`}
          value={source.gainDb}
          min={-30}
          max={12}
          step={0.5}
          onChange={(gainDb) => onChange({ ...source, gainDb })}
        />
      </div>
      <span className="w-14 shrink-0 text-right font-mono text-xs text-ink-muted tabular-nums">
        {source.gainDb > 0 ? "+" : ""}
        {source.gainDb.toFixed(1)} dB
      </span>

      <button
        onClick={() => onChange({ ...source, muted: !source.muted }, true)}
        aria-pressed={source.muted}
        title={source.muted ? "Wieder aufnehmen" : "Stumm — kommt nicht in den Clip"}
        // `cb-btn` und nicht `cb-tool`: die kleinen Werkzeuge werden im
        // Ring-Stil kreisrund, und aus einer Pille mit Beschriftung würde
        // dabei eine Ellipse.
        className={cn(
          "cb-btn h-7 w-10 shrink-0 rounded-pill border text-[11px] font-bold",
          source.muted
            ? "border-live/60 bg-live/15 text-live"
            : "border-line bg-elevated text-ink-muted hover:border-accent hover:text-accent-bright",
        )}
      >
        {source.muted ? "AUS" : "AN"}
      </button>
    </div>
  );
}
