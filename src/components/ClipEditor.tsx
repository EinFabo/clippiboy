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
  /** Seconds, like the player's timeline. */
  start: number;
  end: number;
}

interface Props {
  clip: Clip;
  duration: number;
  tracks: ClipTrack[];
  mix: Record<number, TrackState>;
  /** Is anything on the tracks set to something other than neutral? */
  mixTouched: boolean;
  loadingTracks: boolean;
  onTrack: (index: number, patch: Partial<TrackState>) => void;
  onResetMix: () => void;
  trim: Trim;
  onTrim: (trim: Trim) => void;
  /** Sets start or end to wherever the player currently stands. */
  onMark: (which: "start" | "end") => void;
  /** Are the tracks available separately? Only then can anything be mixed. */
  separateTracks: boolean;
  /** Is anything set differently from what is in the file? */
  dirty: boolean;
  saving: boolean;
  /** 0 to 1 while writing. `null` while there is nothing to report. */
  progress: number | null;
  justSaved: boolean;
  onSave: () => void;
  /** Undo the trim and pull the whole recording back. */
  onRestore: () => void;
  restoring: boolean;
  /** Switch to the audio mixer — that is where the separated tracks come from. */
  onOpenMixer: () => void;
}

/** Range of the track sliders. More than +12 dB only brings distortion. */
const MIN_DB = -30;
const MAX_DB = 12;

/**
 * The editing pane beside the player. It is always open: a clip you are looking
 * at is also the clip you want to name or trim — a toggle in between would only
 * be a click you have to find first.
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
  // A cut at the start has to be frame-accurate — when copying it slid to the
  // keyframe before it. So the video gets recomputed, and that takes time.
  const reencodes = trim.start > 0.25;
  const busy = saving || restoring;
  const anySolo = Object.values(mix).some((state) => state.solo);

  /** Save one field without losing the other two. */
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
      {/* Only the fields scroll — Save stays put at the bottom, otherwise the
          most important button would be the one you have to hunt for. */}
      <div className="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto p-5">
        {/* Name and description carry no label and no border: they look like
            what they are and turn into a field the moment you touch them. An
            edit button in front would be a hurdle before a line of text. */}
        <section>
          <Field
            key={`title-${clip.id}`}
            value={clip.title ?? ""}
            placeholder={fileName(clip.path)}
            ariaLabel="Clip name"
            className="display text-xl leading-snug"
            onSave={(title) => saveMeta({ title })}
          />
          <Field
            key={`description-${clip.id}`}
            value={clip.description ?? ""}
            placeholder="What happens here?"
            ariaLabel="Description"
            multiline
            className="mt-1 text-sm leading-relaxed text-ink-muted"
            onSave={(description) => saveMeta({ description })}
          />
          {/* The game keeps its label: it is not prose but the value the gallery
              filters by. */}
          <label className="mt-3 flex items-center gap-2">
            <span className="shrink-0 text-xs text-ink-faint">Game</span>
            <Field
              key={`game-${clip.id}`}
              value={clip.game ?? ""}
              placeholder="Unknown"
              ariaLabel="Game"
              className="text-sm"
              onSave={(game) => saveMeta({ game })}
            />
          </label>
        </section>

        <section className="space-y-3">
          <div className="flex items-center justify-between">
            <SectionHead>Audio tracks</SectionHead>
            <Reset show={mixTouched} onClick={onResetMix} />
          </div>

          {loadingTracks && (
            <p className="text-xs text-ink-muted">Reading tracks…</p>
          )}

          {tracks.map((track) => (
            <TrackRow
              key={track.index}
              label={track.index === 0 ? "Main mix" : track.label}
              state={mix[track.index] ?? { gainDb: 0, muted: false, solo: false }}
              anySolo={anySolo}
              onChange={(patch) => onTrack(track.index, patch)}
            />
          ))}

          {!loadingTracks && !separateTracks && (
            <div className="space-y-2 rounded-inner bg-elevated p-3">
              <p className="text-xs leading-relaxed text-ink-muted">
                This clip has no separate tracks — microphone and apps are mixed
                in for good and cannot be separated any more. To control them
                individually later, give them their own track in the mixer; that
                applies to the next recordings.
              </p>
              <Button size="sm" onClick={onOpenMixer}>
                Set up in the mixer
              </Button>
            </div>
          )}

          {separateTracks && (
            <p className="text-xs leading-relaxed text-ink-faint">
              The preview can only get quieter — above 0 dB it lowers the other
              tracks instead. On save the level really is raised.
            </p>
          )}
        </section>

        <section className="space-y-3">
          <div className="flex items-center justify-between">
            <SectionHead>Trim</SectionHead>
            <Reset
              show={trimmed}
              onClick={() => onTrim({ start: 0, end: duration })}
            />
          </div>
          <div className="flex gap-2">
            <Button size="sm" className="flex-1" onClick={() => onMark("start")}>
              Start here
            </Button>
            <Button size="sm" className="flex-1" onClick={() => onMark("end")}>
              End here
            </Button>
          </div>
          <p className="font-mono text-xs text-ink-muted tabular-nums">
            {clock(trim.start)} – {clock(trim.end)} · {clock(length)}
          </p>
          {reencodes && (
            <p className="text-xs leading-relaxed text-ink-faint">
              A cut at the start has to be frame-accurate — the video gets
              recomputed for it. That takes longer than usual.
            </p>
          )}

          {clip.original && (
            <div className="space-y-2 rounded-inner bg-elevated p-3">
              <p className="text-xs leading-relaxed text-ink-muted">
                Trimmed from {clock(clip.original.durationMs / 1000)} — starting
                at {clock(clip.original.startMs / 1000)}. The whole recording sits
                beside it and comes back at the press of a button.
              </p>
              <Button
                size="sm"
                disabled={busy || !inTauri}
                onClick={onRestore}
              >
                {restoring ? "Restoring…" : "Undo trim"}
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
          {saving ? "Saving…" : "Save"}
        </Button>
        {busy && progress !== null && <Progress value={progress} />}
        <p className="flex items-start gap-1.5 text-xs leading-relaxed text-ink-faint">
          {/* Mounts only while the message stands, so the stroke draws itself
              anew on every save — an edit in between takes it away again. */}
          {justSaved && !dirty && <DrawnCheck />}
          <span>
          {restoring
            ? "The whole recording is being written back."
            : saving
              ? reencodes
                ? "The clip is being trimmed — the video is recomputed for it."
                : "The clip is being rewritten, the video stays untouched."
              : dirty
                ? "Not saved yet — the file in the folder is still a different one."
                : justSaved
                  ? "Saved. That is how the clip sits in the folder now — ready to send."
                  : clip.original
                    ? "The trim sits in the file. The original is beside it, undoing works any time."
                    : "The file in the folder is exactly what stands here."}
          </span>
        </p>
      </section>
    </aside>
  );
}

/**
 * The check that says it went through — drawn rather than switched on.
 *
 * Same trick the overlay uses for its outline: `pathLength="100"` makes the
 * dash arithmetic independent of how long the stroke actually is, so the line
 * runs itself from start to finish in a set time.
 */
function DrawnCheck() {
  return (
    <svg
      viewBox="0 0 24 24"
      aria-hidden
      className="mt-0.5 h-3.5 w-3.5 shrink-0 text-ok"
      fill="none"
      strokeWidth="2.4"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path className="cb-draw" pathLength={100} d="m5 12.5 4.5 4.5L19 7" />
    </svg>
  );
}

/** A bar instead of a button that only says "wait". */
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

/** Only appears once there is something to take back. */
function Reset({ show, onClick }: { show: boolean; onClick: () => void }) {
  if (!show) return null;
  return (
    <button
      onClick={onClick}
      className="text-xs text-ink-faint transition-colors hover:text-ink"
    >
      Reset
    </button>
  );
}

/** One track: mute it, hear it alone, set its level. */
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
  // Solo beats mute — with solo set anywhere, the others are silent no matter
  // what their own switch says.
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
          aria-label={state.muted ? `Unmute ${label}` : `Mute ${label}`}
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
          Solo
        </button>
      </div>

      <div className="mt-3 flex items-center gap-3">
        <Slider
          label={`Volume ${label}`}
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
 * A text field with no label and no border: it shows the value, and clicking
 * into it changes that value. It saves shortly after the last keystroke — and
 * immediately on leaving, so a quick close swallows nothing.
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
   * What last went to the core — or came from it. Always in the form the core
   * stores, i.e. without surrounding whitespace.
   */
  const saved = useRef(value);
  // The caller passes in a new function on every render; if the timer hung off
  // that, it would restart constantly and never fire.
  const latest = useRef(onSave);
  latest.current = onSave;

  // Changed from outside (different clip, reset): pull the draft along.
  //
  // Our own save comes back in here too, only trimmed. That one must not touch
  // the draft: otherwise the space that was meant to go between two words
  // disappears mid-typing.
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

  // If the field disappears before the timer has run out, what was typed would
  // otherwise be gone — the player closes faster than 500 ms pass.
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

/** A text field that grows with its content instead of getting a scrollbar of
    its own — two sentences of description should be readable without scrolling. */
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
