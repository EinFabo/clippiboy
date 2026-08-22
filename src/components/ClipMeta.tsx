import { useEffect, useLayoutEffect, useRef, useState } from "react";

import { useEngine } from "@/store";
import { fileName } from "@/lib/format";
import { cn } from "@/lib/cn";
import type { Clip } from "@/lib/types";

/**
 * Name, description and game — the three things every clip has, whether it is a
 * recording or a still.
 *
 * Sits in a file of its own because both viewers show it: the player has a whole
 * editing pane around it, the screenshot viewer nothing but this.
 */
export function ClipMeta({ clip }: { clip: Clip }) {
  const updateClip = useEngine((state) => state.updateClip);

  /** Save one field without losing the other two. */
  const saveMeta = (
    patch: Partial<Record<"title" | "description" | "game", string | null>>,
  ) =>
    updateClip(clip.id, {
      title: clip.title,
      description: clip.description,
      game: clip.game,
      ...patch,
    });

  // Name and description carry no label and no border: they look like
  // what they are and turn into a field the moment you touch them. An
  // edit button in front would be a hurdle before a line of text.
  return (
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
