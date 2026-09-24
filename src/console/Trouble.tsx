import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "@/lib/ipc";
import type { AudioSource, EngineStatus } from "@/lib/types";

/**
 * Der Warnstreifen über dem Dock — Vorschlag k02 aus dem Konsolen-Labor.
 *
 * Die häufigste Beschwerde über Aufnahmewerkzeuge ist nicht, dass etwas
 * schiefgeht, sondern dass es still schiefgeht: gedrückt, nichts passiert. Oder
 * der Clip liegt da und das Mikrofon ist eine Stunde lang nicht darin gewesen.
 * Das Hauptfenster sagt das alles längst (`SourceTrouble`, die Warnkarte im
 * Dashboard) — nur sieht es niemand, der gerade spielt.
 *
 * Kostet kein Backend: `audio-errors` und `audio-warnings` gehen an **jedes**
 * Fenster (`lib.rs:868/880`), und der falsche Bildschirm steht als
 * `screenFallback` schon im Status.
 */

export interface Trouble {
  /** Rot statt Gelb: hier fehlt etwas im Clip, es ist nicht nur ungewöhnlich. */
  severe: boolean;
  text: string;
}

/** Wie viele Zeilen der Streifen höchstens trägt. Darüber wird er zur Wand, und
 *  eine Wand über dem Spiel liest niemand. Die Zahl steht auch in console.css
 *  (`--warn-lift`), weil die Notiz über dem Streifen Platz braucht. */
export const TROUBLE_LINES = 3;

/** Hört auf die beiden Meldungskanäle des Kerns. Eigener Haken und nicht im
 *  großen `useEffect` der Konsole: die beiden gehören zusammen und sonst
 *  nirgendwo hin. */
export function useAudioTrouble() {
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [warnings, setWarnings] = useState<Record<string, string>>({});

  useEffect(() => {
    if (!inTauri) return;
    const offs = [
      listen<Record<string, string>>("audio-errors", (e) => setErrors(e.payload)),
      listen<Record<string, string>>("audio-warnings", (e) => setWarnings(e.payload)),
    ];
    return () => {
      offs.forEach((off) => void off.then((fn) => fn()));
    };
  }, []);

  return { errors, warnings };
}

/**
 * Was gerade schiefläuft, als Liste — das Schlimmste zuerst.
 *
 * Eine reine Funktion, damit die Reihenfolge und die Sätze an einer Stelle
 * stehen und nicht im Markup verteilt sind.
 */
export function troubles({
  status,
  sources,
  errors,
  warnings,
  droppedRecently,
}: {
  status: EngineStatus;
  sources: AudioSource[];
  errors: Record<string, string>;
  warnings: Record<string, string>;
  /** Verworfene Bilder in der letzten Minute — aus dem Verlauf (`useTrend`).
   *  Der Zählerstand aus dem Status taugt dafür nicht: der steht auch dann noch
   *  auf zwölf, wenn die zwölf vor einer Stunde ausgefallen sind. */
  droppedRecently: number;
}): Trouble[] {
  const out: Trouble[] = [];

  // Eine Quelle, deren Strom gar nicht erst anläuft. Die Spur wird trotzdem
  // geschrieben und ist dann still — das merkt man beim Anschauen, also Stunden
  // zu spät.
  for (const source of sources) {
    if (source.enabled && errors[source.id]) {
      out.push({
        severe: true,
        text: `${source.label} nimmt nicht auf — die Spur bleibt im Clip still`,
      });
    }
  }

  if (status.screenFallback) {
    out.push({
      severe: false,
      text: `Der gewählte Bildschirm ist nicht da — aufgenommen wird ${status.screenFallback}`,
    });
  }

  // Quellen, die laufen, aber nicht das aufnehmen, was ihr Name verspricht. Der
  // Satz kommt vom Kern und schließt an den Namen an — genauso setzt ihn das
  // Hauptfenster zusammen (`SourceTrouble`).
  for (const source of sources) {
    if (source.enabled && !errors[source.id] && warnings[source.id]) {
      out.push({ severe: false, text: `${source.label} ${warnings[source.id]}` });
    }
  }

  if (droppedRecently > 0) {
    out.push({
      severe: false,
      text: `${droppedRecently} ${droppedRecently === 1 ? "Bild" : "Bilder"} verworfen in der letzten Minute — der Encoder kommt nicht mit`,
    });
  }

  return out;
}

/** Der Streifen selbst. Er steht direkt über dem Dock und geht dessen Weg mit —
 *  siehe `.cb-warn` in console.css. */
export function TroubleStrip({ list }: { list: Trouble[] }) {
  if (list.length === 0) return null;
  // Passt nicht alles, macht die letzte Zeile die Sammelmeldung — der Streifen
  // wird also nie höher als `TROUBLE_LINES`, egal wie viel schiefgeht.
  const rest = list.length > TROUBLE_LINES ? list.length - (TROUBLE_LINES - 1) : 0;
  const shown = list.slice(0, rest > 0 ? TROUBLE_LINES - 1 : TROUBLE_LINES);
  return (
    // Gelesen, nicht bedient: der Klick darauf soll dasselbe heißen wie der
    // Klick daneben — die Konsole zu. Dieselbe Regel wie bei der Hilfszeile
    // unter dem Dock.
    <div
      className="cb-warn pointer-events-none"
      data-severe={list.some((t) => t.severe)}
      role="status"
    >
      {shown.map((trouble, i) => (
        // Der Index als Schlüssel: die Animation sitzt auf dem Streifen und
        // nicht auf den Zeilen, es hängt also nichts daran, ob React eine
        // Zeile wiedererkennt — und zwei Quellen dürfen gleich heißen.
        <p key={i} className="cb-warn-line">
          <span aria-hidden="true">{trouble.severe ? "●" : "▲"}</span> {trouble.text}
        </p>
      ))}
      {rest > 0 && (
        <p className="cb-warn-line cb-warn-rest">
          und {rest} {rest === 1 ? "weitere Meldung" : "weitere Meldungen"} — im Fenster
          nachsehen
        </p>
      )}
    </div>
  );
}
