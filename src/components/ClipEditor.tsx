import { useLayoutEffect, useRef, useState, useEffect } from "react";

import { Button } from "@/components/ui/Button";
import { Slider } from "@/components/ui/Controls";
import { IconSpeaker } from "@/components/icons";
import { useEngine } from "@/store";
import { inTauri } from "@/lib/ipc";
import { fileName } from "@/lib/format";
import { cn } from "@/lib/cn";
import type { TrackState } from "@/lib/useClipMix";
import type { Clip, ClipTrack } from "@/lib/types";

export interface Trim {
  /** Sekunden, wie die Zeitachse des Players. */
  start: number;
  end: number;
}

interface Props {
  clip: Clip;
  duration: number;
  tracks: ClipTrack[];
  mix: Record<number, TrackState>;
  /** Steht an den Spuren etwas anderes als neutral? */
  mixTouched: boolean;
  loadingTracks: boolean;
  onTrack: (index: number, patch: Partial<TrackState>) => void;
  onResetMix: () => void;
  trim: Trim;
  onTrim: (trim: Trim) => void;
  /** Setzt Anfang oder Ende auf die Stelle, an der der Player gerade steht. */
  onMark: (which: "start" | "end") => void;
  /** Liegen die Spuren einzeln vor? Nur dann lässt sich überhaupt mischen. */
  separateTracks: boolean;
  /** Steht etwas anderes eingestellt als in der Datei? */
  dirty: boolean;
  saving: boolean;
  /** 0 bis 1, solange geschrieben wird. `null`, solange nichts zu melden ist. */
  progress: number | null;
  justSaved: boolean;
  onSave: () => void;
  /** Den Zuschnitt aufheben und die ganze Aufnahme zurückholen. */
  onRestore: () => void;
  restoring: boolean;
  /** Zum Audio-Mixer wechseln — von dort kommen die getrennten Spuren. */
  onOpenMixer: () => void;
}

/** Regelbereich der Spurenregler. Mehr als +12 dB bringt nur Verzerrung. */
const MIN_DB = -30;
const MAX_DB = 12;

/**
 * Der Bearbeiten-Bereich neben dem Player. Er ist immer offen: Ein Clip, den
 * man gerade ansieht, ist auch der Clip, den man benennen oder schneiden will
 * — ein Umschalter dazwischen wäre nur ein Klick, den man erst finden muss.
 */
export function ClipEditor({
  clip,
  duration,
  tracks,
  mix,
  mixTouched,
  loadingTracks,
  onTrack,
  onResetMix,
  trim,
  onTrim,
  onMark,
  separateTracks,
  dirty,
  saving,
  progress,
  justSaved,
  onSave,
  onRestore,
  restoring,
  onOpenMixer,
}: Props) {
  const updateClip = useEngine((state) => state.updateClip);

  const trimmed = trim.start > 0.05 || trim.end < duration - 0.05;
  const length = Math.max(0, trim.end - trim.start);
  // Ein Schnitt am Anfang muss bildgenau sitzen — beim Kopieren rutschte er auf
  // das Keyframe davor. Dafür wird das Bild neu gerechnet, und das dauert.
  const reencodes = trim.start > 0.25;
  const busy = saving || restoring;
  const anySolo = Object.values(mix).some((state) => state.solo);

  /** Ein Feld speichern, ohne die beiden anderen zu verlieren. */
  const saveMeta = (patch: Partial<Record<"title" | "description" | "game", string | null>>) =>
    updateClip(clip.id, {
      title: clip.title,
      description: clip.description,
      game: clip.game,
      ...patch,
    });

  return (
    <aside
      className="flex w-[380px] shrink-0 flex-col rounded-card border border-line
        bg-surface/80 backdrop-blur-xl"
    >
      {/* Nur die Felder scrollen — Speichern bleibt unten stehen, sonst wäre
          der wichtigste Knopf ausgerechnet der, den man suchen muss. */}
      <div className="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto p-5">
        {/* Name und Beschreibung tragen keine Beschriftung und keinen Rahmen:
            Sie sehen aus wie das, was sie sind, und werden zum Feld, sobald
            man sie anfasst. Ein Bearbeiten-Knopf davor wäre eine Hürde vor
            einer Textzeile. */}
        <section>
          <Field
            key={`title-${clip.id}`}
            value={clip.title ?? ""}
            placeholder={fileName(clip.path)}
            ariaLabel="Name des Clips"
            className="display text-xl leading-snug"
            onSave={(title) => saveMeta({ title })}
          />
          <Field
            key={`description-${clip.id}`}
            value={clip.description ?? ""}
            placeholder="Was passiert hier?"
            ariaLabel="Beschreibung"
            multiline
            className="mt-1 text-sm leading-relaxed text-ink-muted"
            onSave={(description) => saveMeta({ description })}
          />
          {/* Das Spiel bleibt beschriftet: Es ist kein Fließtext, sondern der
              Wert, nach dem die Galerie filtert. */}
          <label className="mt-3 flex items-center gap-2">
            <span className="shrink-0 text-xs text-ink-faint">Spiel</span>
            <Field
              key={`game-${clip.id}`}
              value={clip.game ?? ""}
              placeholder="Unbekannt"
              ariaLabel="Spiel"
              className="text-sm"
              onSave={(game) => saveMeta({ game })}
            />
          </label>
        </section>

        <section className="space-y-3">
          <div className="flex items-center justify-between">
            <SectionHead>Tonspuren</SectionHead>
            <Reset show={mixTouched} onClick={onResetMix} />
          </div>

          {loadingTracks && (
            <p className="text-xs text-ink-muted">Spuren werden gelesen…</p>
          )}

          {tracks.map((track) => (
            <TrackRow
              key={track.index}
              label={track.index === 0 ? "Hauptmix" : track.label}
              state={mix[track.index] ?? { gainDb: 0, muted: false, solo: false }}
              anySolo={anySolo}
              onChange={(patch) => onTrack(track.index, patch)}
            />
          ))}

          {!loadingTracks && !separateTracks && (
            <div className="space-y-2 rounded-inner bg-elevated p-3">
              <p className="text-xs leading-relaxed text-ink-muted">
                Von diesem Clip gibt es keine Einzelspuren — Mikrofon und Apps
                sind fest eingemischt und lassen sich nicht mehr trennen. Wer
                sie später einzeln regeln will, gibt ihnen im Mixer eine eigene
                Spur; das gilt dann für die nächsten Aufnahmen.
              </p>
              <Button size="sm" onClick={onOpenMixer}>
                Im Mixer einrichten
              </Button>
            </div>
          )}

          {separateTracks && (
            <p className="text-xs leading-relaxed text-ink-faint">
              Die Vorschau kann nur leiser werden — über 0 dB senkt sie
              stattdessen die übrigen Spuren ab. Beim Speichern wird der Pegel
              wirklich angehoben.
            </p>
          )}
        </section>

        <section className="space-y-3">
          <div className="flex items-center justify-between">
            <SectionHead>Zuschnitt</SectionHead>
            <Reset
              show={trimmed}
              onClick={() => onTrim({ start: 0, end: duration })}
            />
          </div>
          <div className="flex gap-2">
            <Button size="sm" className="flex-1" onClick={() => onMark("start")}>
              Start hier
            </Button>
            <Button size="sm" className="flex-1" onClick={() => onMark("end")}>
              Ende hier
            </Button>
          </div>
          <p className="font-mono text-xs text-ink-muted tabular-nums">
            {clock(trim.start)} – {clock(trim.end)} · {clock(length)}
          </p>
          {reencodes && (
            <p className="text-xs leading-relaxed text-ink-faint">
              Ein Schnitt am Anfang muss bildgenau sitzen — dafür wird das Bild
              neu gerechnet. Das dauert länger als sonst.
            </p>
          )}

          {clip.original && (
            <div className="space-y-2 rounded-inner bg-elevated p-3">
              <p className="text-xs leading-relaxed text-ink-muted">
                Geschnitten aus {clock(clip.original.durationMs / 1000)} — ab{" "}
                {clock(clip.original.startMs / 1000)}. Die ganze Aufnahme liegt
                daneben und kommt auf Knopfdruck zurück.
              </p>
              <Button
                size="sm"
                disabled={busy || !inTauri}
                onClick={onRestore}
              >
                {restoring ? "Wird zurückgeholt…" : "Zuschnitt aufheben"}
              </Button>
            </div>
          )}
        </section>
      </div>

      <section className="space-y-3 border-t border-line p-5">
        <Button
          variant="primary"
          className="w-full"
          disabled={busy || !dirty || !inTauri}
          onClick={onSave}
        >
          {saving ? "Wird gespeichert…" : "Speichern"}
        </Button>
        {busy && progress !== null && <Progress value={progress} />}
        <p className="text-xs leading-relaxed text-ink-faint">
          {restoring
            ? "Die ganze Aufnahme wird zurückgeschrieben."
            : saving
              ? reencodes
                ? "Der Clip wird geschnitten — das Bild wird dafür neu gerechnet."
                : "Der Clip wird neu geschrieben, das Bild bleibt unangetastet."
              : dirty
                ? "Noch nicht gespeichert — die Datei im Ordner ist bisher eine andere."
                : justSaved
                  ? "Gespeichert. So liegt der Clip jetzt im Ordner — fertig zum Verschicken."
                  : clip.original
                    ? "Der Zuschnitt steckt in der Datei. Das Original liegt daneben, aufheben geht jederzeit."
                    : "Die Datei im Ordner ist genau das, was hier steht."}
        </p>
      </section>
    </aside>
  );
}

/** Ein Balken statt eines Knopfes, der nur „warte" sagt. */
function Progress({ value }: { value: number }) {
  return (
    <div className="h-1 overflow-hidden rounded-pill bg-white/10">
      <div
        className="h-full rounded-pill bg-accent transition-[width] duration-200"
        style={{ width: `${Math.round(Math.min(1, Math.max(0, value)) * 100)}%` }}
      />
    </div>
  );
}

function SectionHead({ children }: { children: React.ReactNode }) {
  return (
    <h3 className="text-xs font-medium tracking-wide text-ink-faint uppercase">
      {children}
    </h3>
  );
}

/** Erscheint erst, wenn es etwas zurückzunehmen gibt. */
function Reset({ show, onClick }: { show: boolean; onClick: () => void }) {
  if (!show) return null;
  return (
    <button
      onClick={onClick}
      className="text-xs text-ink-faint transition-colors hover:text-ink"
    >
      Zurücksetzen
    </button>
  );
}

/** Eine Spur: stumm schalten, alleine hören, aussteuern. */
function TrackRow({
  label,
  state,
  anySolo,
  onChange,
}: {
  label: string;
  state: TrackState;
  anySolo: boolean;
  onChange: (patch: Partial<TrackState>) => void;
}) {
  // Solo schlägt Stumm — steht irgendwo Solo, sind die anderen still, ganz
  // gleich, was ihr eigener Schalter sagt.
  const silent = anySolo ? !state.solo : state.muted;

  return (
    <div
      className={cn(
        "rounded-inner bg-elevated p-3 transition-opacity",
        silent && "opacity-55",
      )}
    >
      <div className="flex items-center gap-2">
        <button
          aria-label={state.muted ? `${label} einschalten` : `${label} stumm`}
          aria-pressed={state.muted}
          onClick={() => onChange({ muted: !state.muted })}
          className={cn(
            "grid h-8 w-8 shrink-0 place-items-center rounded-pill transition-colors",
            state.muted
              ? "bg-live/20 text-live"
              : "bg-white/10 text-ink-muted hover:text-ink",
          )}
        >
          {state.muted ? (
            <svg viewBox="0 0 24 24" className="h-[18px] w-[18px]" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinejoin="round">
              <path d="M4 9.5h3.5L12 5.5v13L7.5 14.5H4v-5Z" />
              <path d="M16 10l4 4M20 10l-4 4" strokeLinecap="round" />
            </svg>
          ) : (
            <IconSpeaker className="h-[18px] w-[18px]" />
          )}
        </button>

        <span className="min-w-0 flex-1 truncate text-sm">{label}</span>

        <button
          aria-pressed={state.solo}
          onClick={() => onChange({ solo: !state.solo })}
          className={cn(
            "shrink-0 rounded-pill px-2.5 py-1 text-[11px] font-medium transition-colors",
            state.solo
              ? "bg-accent text-white"
              : "bg-white/10 text-ink-faint hover:text-ink",
          )}
        >
          Nur diese
        </button>
      </div>

      <div className="mt-3 flex items-center gap-3">
        <Slider
          label={`Lautstärke ${label}`}
          value={state.gainDb}
          min={MIN_DB}
          max={MAX_DB}
          step={0.5}
          onChange={(gainDb) => onChange({ gainDb })}
        />
        <span className="w-14 shrink-0 text-right font-mono text-xs text-ink-muted tabular-nums">
          {state.gainDb > 0 ? "+" : ""}
          {state.gainDb.toFixed(1)} dB
        </span>
      </div>
    </div>
  );
}

/**
 * Textfeld ohne Beschriftung und ohne Rahmen: Es zeigt den Wert, und wer
 * hineinklickt, ändert ihn. Gespeichert wird kurz nach dem letzten Anschlag —
 * und beim Verlassen sofort, damit ein schnelles Schließen nichts frisst.
 */
function Field({
  value,
  placeholder,
  ariaLabel,
  multiline,
  className,
  onSave,
}: {
  value: string;
  placeholder?: string;
  ariaLabel: string;
  multiline?: boolean;
  className?: string;
  onSave: (value: string | null) => void;
}) {
  const [draft, setDraft] = useState(value);
  /**
   * Was zuletzt an den Kern ging — oder von dort kam. Immer in der Form, die
   * der Kern ablegt, also ohne Leerzeichen am Rand.
   */
  const saved = useRef(value);
  // Der Aufrufer gibt bei jedem Render eine neue Funktion herein; hinge der
  // Timer daran, würde er dauernd neu anfangen und nie auslösen.
  const latest = useRef(onSave);
  latest.current = onSave;

  // Von außen geändert (anderer Clip, Zurücksetzen): Entwurf nachziehen.
  //
  // Die eigene Speicherung kommt hier ebenfalls wieder herein, nur eben
  // getrimmt. Die darf den Entwurf nicht anfassen: Sonst verschwindet mitten
  // im Tippen das Leerzeichen, das gerade zwischen zwei Wörter sollte.
  useEffect(() => {
    if (value === saved.current) return;
    saved.current = value;
    setDraft(value);
  }, [value]);

  const commit = () => {
    const next = draft.trim();
    if (next === saved.current) return;
    saved.current = next;
    latest.current(next ? next : null);
  };
  const commitRef = useRef(commit);
  commitRef.current = commit;

  useEffect(() => {
    if (draft === saved.current) return;
    const timer = window.setTimeout(() => commitRef.current(), 500);
    return () => window.clearTimeout(timer);
  }, [draft]);

  // Verschwindet das Feld, bevor der Timer abgelaufen ist, wäre das Getippte
  // sonst weg — der Player schließt schneller, als 500 ms vergehen.
  useEffect(() => () => commitRef.current(), []);

  const shared = cn(
    "w-full rounded-inner border border-transparent bg-transparent px-2 py-1.5 outline-none",
    "transition-colors placeholder:text-ink-faint",
    "hover:border-line focus:border-line-strong focus:bg-elevated",
    className,
  );

  if (!multiline) {
    return (
      <input
        aria-label={ariaLabel}
        value={draft}
        placeholder={placeholder}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        className={shared}
      />
    );
  }
  return (
    <GrowingArea
      ariaLabel={ariaLabel}
      value={draft}
      placeholder={placeholder}
      onChange={setDraft}
      onBlur={commit}
      className={shared}
    />
  );
}

/** Textfeld, das mit seinem Inhalt wächst statt eine eigene Bildlaufleiste zu
    bekommen — zwei Sätze Beschreibung sollen ohne Scrollen lesbar sein. */
function GrowingArea({
  ariaLabel,
  value,
  placeholder,
  onChange,
  onBlur,
  className,
}: {
  ariaLabel: string;
  value: string;
  placeholder?: string;
  onChange: (value: string) => void;
  onBlur: () => void;
  className?: string;
}) {
  const area = useRef<HTMLTextAreaElement>(null);

  useLayoutEffect(() => {
    const element = area.current;
    if (!element) return;
    element.style.height = "auto";
    element.style.height = `${element.scrollHeight}px`;
  }, [value]);

  return (
    <textarea
      ref={area}
      aria-label={ariaLabel}
      value={value}
      rows={1}
      placeholder={placeholder}
      onChange={(e) => onChange(e.target.value)}
      onBlur={onBlur}
      className={cn(className, "resize-none overflow-hidden")}
    />
  );
}

function clock(seconds: number): string {
  if (!Number.isFinite(seconds)) return "0:00";
  const total = Math.max(0, Math.floor(seconds));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}
