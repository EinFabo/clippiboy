import { useEffect, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";

import { Button } from "@/components/ui/Button";
import { Slider } from "@/components/ui/Controls";
import { useEngine } from "@/store";
import { api, events, inTauri } from "@/lib/ipc";
import { formatSize } from "@/lib/format";
import { cn } from "@/lib/cn";
import type { TrackState } from "@/lib/useClipMix";
import type { Clip, ClipTrack, ExportResult, TrackMix } from "@/lib/types";

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
  loadingTracks: boolean;
  onTrack: (index: number, patch: Partial<TrackState>) => void;
  onResetMix: () => void;
  trim: Trim;
  onTrim: (trim: Trim) => void;
  /** Setzt Anfang oder Ende auf die Stelle, an der der Player gerade steht. */
  onMark: (which: "start" | "end") => void;
  toRequest: () => TrackMix[];
}

/** Regelbereich der Spurenregler. Mehr als +12 dB bringt nur Verzerrung. */
const MIN_DB = -30;
const MAX_DB = 12;

export function ClipEditor({
  clip,
  duration,
  tracks,
  mix,
  loadingTracks,
  onTrack,
  onResetMix,
  trim,
  onTrim,
  onMark,
  toRequest,
}: Props) {
  const updateClip = useEngine((state) => state.updateClip);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState(0);
  const [done, setDone] = useState<ExportResult | null>(null);

  const trimmed = trim.start > 0.05 || trim.end < duration - 0.05;
  const length = Math.max(0, trim.end - trim.start);

  useEffect(() => {
    setDone(null);
  }, [clip.id]);

  useEffect(() => {
    if (!inTauri) return;
    const unlisten = events.onExportProgress((update) => {
      if (update.clipId === clip.id) setProgress(update.progress);
    });
    return () => void unlisten.then((off) => off());
  }, [clip.id]);

  async function runExport() {
    const target = await save({
      title: "Clip exportieren",
      defaultPath: suggestedPath(clip),
      filters: [{ name: "MP4-Video", extensions: ["mp4"] }],
    });
    if (!target) return;

    setBusy(true);
    setProgress(0);
    setDone(null);
    try {
      setDone(
        await api.exportClip({
          clipId: clip.id,
          startMs: Math.round(trim.start * 1000),
          endMs: Math.round(trim.end * 1000),
          tracks: toRequest(),
          output: target,
        }),
      );
    } catch {
      // Der Kern meldet den Fehler bereits als Toast.
    } finally {
      setBusy(false);
    }
  }

  return (
    <aside
      className="flex w-[360px] shrink-0 flex-col gap-6 overflow-y-auto rounded-card border
        border-line bg-surface/80 p-5 backdrop-blur-xl"
    >
      <section className="space-y-3">
        <h3 className="text-xs font-medium tracking-wide text-ink-faint uppercase">
          Clip
        </h3>
        <Field
          label="Name"
          value={clip.title ?? ""}
          placeholder={fileName(clip)}
          onSave={(title) =>
            updateClip(clip.id, {
              title,
              description: clip.description,
              game: clip.game,
            })
          }
        />
        <Field
          label="Beschreibung"
          value={clip.description ?? ""}
          placeholder="Was passiert hier?"
          multiline
          onSave={(description) =>
            updateClip(clip.id, {
              title: clip.title,
              description,
              game: clip.game,
            })
          }
        />
        <Field
          label="Spiel"
          value={clip.game ?? ""}
          placeholder="Unbekannt"
          onSave={(game) =>
            updateClip(clip.id, {
              title: clip.title,
              description: clip.description,
              game,
            })
          }
        />
      </section>

      <section className="space-y-3">
        <div className="flex items-center justify-between">
          <h3 className="text-xs font-medium tracking-wide text-ink-faint uppercase">
            Tonspuren
          </h3>
          <button
            onClick={onResetMix}
            className="text-xs text-ink-faint transition-colors hover:text-ink"
          >
            Zurücksetzen
          </button>
        </div>

        {loadingTracks && (
          <p className="text-xs text-ink-muted">Spuren werden gelesen…</p>
        )}
        {!loadingTracks && tracks.length === 0 && (
          <p className="text-xs text-ink-muted">
            Dieser Clip hat keine getrennten Tonspuren.
          </p>
        )}

        {tracks.map((track) => {
          const state = mix[track.index] ?? { gainDb: 0, muted: false };
          return (
            <div key={track.index} className="rounded-inner bg-elevated p-3">
              <div className="flex items-center gap-2">
                <button
                  aria-label={state.muted ? "Spur einschalten" : "Spur stumm"}
                  onClick={() => onTrack(track.index, { muted: !state.muted })}
                  className={cn(
                    "grid h-7 w-7 shrink-0 place-items-center rounded-pill text-[11px] font-semibold",
                    "transition-colors",
                    state.muted
                      ? "bg-live/20 text-live"
                      : "bg-white/10 text-ink-muted hover:text-ink",
                  )}
                >
                  M
                </button>
                <span className="min-w-0 flex-1 truncate text-sm">
                  {track.label}
                </span>
                <span
                  className={cn(
                    "shrink-0 font-mono text-xs tabular-nums",
                    state.muted ? "text-ink-faint" : "text-ink-muted",
                  )}
                >
                  {state.gainDb > 0 ? "+" : ""}
                  {state.gainDb.toFixed(1)} dB
                </span>
              </div>
              <div className="mt-3">
                <Slider
                  label={`Lautstärke ${track.label}`}
                  value={state.gainDb}
                  min={MIN_DB}
                  max={MAX_DB}
                  step={0.5}
                  onChange={(gainDb) => onTrack(track.index, { gainDb })}
                />
              </div>
            </div>
          );
        })}

        {tracks.length > 1 && (
          <p className="text-xs leading-relaxed text-ink-faint">
            Die Vorschau kann nur leiser werden — steht ein Regler über 0 dB,
            senkt sie die übrigen Spuren entsprechend ab. Der Export hebt den
            Pegel wirklich an.
          </p>
        )}
      </section>

      <section className="space-y-3">
        <h3 className="text-xs font-medium tracking-wide text-ink-faint uppercase">
          Zuschnitt
        </h3>
        <div className="flex gap-2">
          <Button size="sm" className="flex-1" onClick={() => onMark("start")}>
            Start hier
          </Button>
          <Button size="sm" className="flex-1" onClick={() => onMark("end")}>
            Ende hier
          </Button>
        </div>
        <div className="flex items-center justify-between text-xs text-ink-muted">
          <span className="font-mono tabular-nums">
            {clock(trim.start)} – {clock(trim.end)} · {clock(length)}
          </span>
          {trimmed && (
            <button
              onClick={() => onTrim({ start: 0, end: duration })}
              className="text-ink-faint transition-colors hover:text-ink"
            >
              Ganzer Clip
            </button>
          )}
        </div>
        {trimmed && (
          <p className="text-xs leading-relaxed text-ink-faint">
            Ein Schnitt am Anfang macht den Export bildgenau — dafür wird das
            Bild neu berechnet und das dauert länger.
          </p>
        )}
      </section>

      <section className="mt-auto space-y-3 border-t border-line pt-4">
        <Button
          variant="primary"
          className="w-full"
          disabled={busy || !inTauri}
          onClick={runExport}
        >
          {busy ? `Wird exportiert… ${Math.round(progress * 100)} %` : "Exportieren"}
        </Button>
        {busy && (
          <div className="h-1 overflow-hidden rounded-pill bg-line">
            <div
              className="h-full rounded-pill bg-accent-bright transition-[width] duration-200"
              style={{ width: `${Math.max(2, progress * 100)}%` }}
            />
          </div>
        )}
        {!busy && (
          <p className="text-xs leading-relaxed text-ink-faint">
            Der Export rechnet alle Spuren mit ihren Reglern zu einer Tonspur
            zusammen — die Datei klingt dann überall so wie hier.
          </p>
        )}
        {done && (
          <div className="flex items-center justify-between gap-2 rounded-inner bg-elevated p-3">
            <div className="min-w-0">
              <p className="truncate text-xs font-medium">
                {done.path.split(/[\\/]/).pop()}
              </p>
              <p className="text-xs text-ink-faint">{formatSize(done.sizeBytes)}</p>
            </div>
            <Button size="sm" onClick={() => api.revealPath(done.path)}>
              Zeigen
            </Button>
          </div>
        )}
      </section>
    </aside>
  );
}

/**
 * Textfeld, das beim Tippen sofort reagiert und die Änderung kurz danach
 * speichert — jeder Anschlag einzeln in die Datenbank wäre Unsinn.
 */
function Field({
  label,
  value,
  placeholder,
  multiline,
  onSave,
}: {
  label: string;
  value: string;
  placeholder?: string;
  multiline?: boolean;
  onSave: (value: string | null) => void;
}) {
  const [draft, setDraft] = useState(value);
  const saved = useRef(value);
  // Der Aufrufer gibt bei jedem Render eine neue Funktion herein; hinge der
  // Timer daran, würde er dauernd neu anfangen und nie auslösen.
  const latest = useRef(onSave);
  latest.current = onSave;

  // Von außen geändert (anderer Clip, Speicherung durch): Entwurf nachziehen.
  useEffect(() => {
    setDraft(value);
    saved.current = value;
  }, [value]);

  useEffect(() => {
    if (draft === saved.current) return;
    const timer = window.setTimeout(() => {
      saved.current = draft;
      latest.current(draft.trim() ? draft.trim() : null);
    }, 500);
    return () => window.clearTimeout(timer);
  }, [draft]);

  const className = cn(
    "w-full rounded-inner border border-line bg-elevated px-3 py-2 text-sm outline-none",
    "transition-colors placeholder:text-ink-faint focus:border-line-strong",
  );

  return (
    <label className="block">
      <span className="mb-1.5 block text-xs text-ink-muted">{label}</span>
      {multiline ? (
        <textarea
          value={draft}
          rows={3}
          placeholder={placeholder}
          onChange={(e) => setDraft(e.target.value)}
          className={cn(className, "resize-none")}
        />
      ) : (
        <input
          value={draft}
          placeholder={placeholder}
          onChange={(e) => setDraft(e.target.value)}
          className={className}
        />
      )}
    </label>
  );
}

function fileName(clip: Clip): string {
  return clip.path.split(/[\\/]/).pop() ?? clip.path;
}

/** Vorschlag für den Speichern-Dialog: Name des Clips neben der Vorlage. */
function suggestedPath(clip: Clip): string {
  const separator = clip.path.includes("\\") ? "\\" : "/";
  const folder = clip.path.slice(0, clip.path.lastIndexOf(separator));
  const base = clip.title
    ? clip.title.replace(/[\\/:*?"<>|]/g, "_").trim()
    : fileName(clip).replace(/\.[^.]+$/, "");
  return `${folder}${separator}${base}_export.mp4`;
}

function clock(seconds: number): string {
  if (!Number.isFinite(seconds)) return "0:00";
  const total = Math.max(0, Math.floor(seconds));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}
