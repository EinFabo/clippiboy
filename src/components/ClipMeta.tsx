import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { useEngine } from "@/store";
import { fileName } from "@/lib/format";
import { cn } from "@/lib/cn";
import { IconClose } from "@/components/icons";
import type { Clip } from "@/lib/types";

/**
 * Name, description, game and tags — what every clip has, whether it is a
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
      <Tags key={`tags-${clip.id}`} clip={clip} />
    </section>
  );
}

/** How many suggestions stand under the field at most. */
const SUGGEST = 6;

/**
 * The clip's tags as chips, and a field to add one. Free text — but what has
 * been used before is offered while typing, and a tag that already exists in
 * another spelling takes that one, so "Clutch" and "clutch" never stand side by
 * side in the gallery's filter row.
 *
 * Enter or a comma adds, Backspace in the empty field takes the last one back,
 * Escape leaves. Saved at once: a tag is a click, not a text still being typed.
 */
function Tags({ clip }: { clip: Clip }) {
  const clips = useEngine((state) => state.clips);
  const setClipTags = useEngine((state) => state.setClipTags);
  const [draft, setDraft] = useState("");
  const [focused, setFocused] = useState(false);
  const [pick, setPick] = useState(-1);
  const field = useRef<HTMLInputElement>(null);
  /** Escape means "not this one". The blur it causes runs before the emptied
   *  draft has rendered, and would otherwise still add what was typed. */
  const dropping = useRef(false);

  /** Every tag in the library, the most used first. */
  const known = useMemo(() => {
    const counts = new Map<string, number>();
    for (const c of clips) {
      for (const tag of c.tags) counts.set(tag, (counts.get(tag) ?? 0) + 1);
    }
    return [...counts]
      .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0], "en"))
      .map(([tag]) => tag);
  }, [clips]);

  const own = new Set(clip.tags.map((tag) => tag.toLowerCase()));
  const typed = draft.trim().replace(/^#+/, "").trim().toLowerCase();
  // Beginning with what was typed comes before merely containing it.
  const suggestions = known
    .filter((tag) => !own.has(tag.toLowerCase()))
    .filter((tag) => tag.toLowerCase().includes(typed))
    .sort(
      (a, b) =>
        Number(!a.toLowerCase().startsWith(typed)) -
        Number(!b.toLowerCase().startsWith(typed)),
    )
    .slice(0, SUGGEST);
  const open = focused && suggestions.length > 0;

  // A new list means the old position points at something else.
  useEffect(() => setPick(-1), [typed]);

  const add = (raw: string) => {
    const bare = raw.trim().replace(/^#+/, "").trim();
    setDraft("");
    if (!bare) return;
    const existing = known.find((tag) => tag.toLowerCase() === bare.toLowerCase());
    const tag = existing ?? bare;
    if (own.has(tag.toLowerCase())) return;
    void setClipTags(clip.id, [...clip.tags, tag]);
  };

  const remove = (tag: string) =>
    void setClipTags(
      clip.id,
      clip.tags.filter((t) => t !== tag),
    );

  const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter" || event.key === ",") {
      event.preventDefault();
      add(pick >= 0 && open ? suggestions[pick] : draft);
    } else if (event.key === "ArrowDown" && open) {
      event.preventDefault();
      setPick((p) => (p + 1) % suggestions.length);
    } else if (event.key === "ArrowUp" && open) {
      event.preventDefault();
      setPick((p) => (p <= 0 ? suggestions.length - 1 : p - 1));
    } else if (event.key === "Backspace" && draft === "" && clip.tags.length > 0) {
      event.preventDefault();
      remove(clip.tags[clip.tags.length - 1]);
    } else if (event.key === "Escape") {
      event.preventDefault();
      dropping.current = true;
      setDraft("");
      field.current?.blur();
    }
  };

  return (
    <div className="mt-2 flex items-start gap-2">
      <span className="shrink-0 pt-2 text-xs text-ink-faint">Tags</span>
      <div className="relative min-w-0 flex-1">
        <div
          // A click on the empty room beside the chips means the field.
          onMouseDown={(event) => {
            if (event.target !== event.currentTarget) return;
            event.preventDefault();
            field.current?.focus();
          }}
          className={cn(
            "flex min-h-[34px] flex-wrap items-center gap-1.5 rounded-inner border px-2 py-1",
            "transition-colors",
            focused
              ? "border-line-strong bg-elevated"
              : "border-transparent hover:border-line",
          )}
        >
          {clip.tags.map((tag) => (
            <span
              key={tag}
              className="flex h-6 items-center gap-1 rounded-pill border border-line bg-surface pl-2.5 pr-1 text-xs text-ink-muted"
            >
              <span className="text-ink-faint">#</span>
              <span className="max-w-[160px] truncate">{tag}</span>
              <button
                aria-label={`Remove the tag "${tag}"`}
                title="Remove tag"
                onClick={() => remove(tag)}
                className="grid h-4 w-4 place-items-center rounded-pill transition hover:bg-hover hover:text-live"
              >
                <IconClose className="h-2.5 w-2.5" />
              </button>
            </span>
          ))}
          <input
            ref={field}
            aria-label="Add a tag"
            value={draft}
            maxLength={32}
            placeholder={clip.tags.length === 0 ? "Add a tag" : ""}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={onKeyDown}
            onFocus={() => setFocused(true)}
            // Whatever stands in the field when leaving still counts — the same
            // promise the other fields make.
            onBlur={() => {
              setFocused(false);
              if (!dropping.current && draft.trim()) add(draft);
              dropping.current = false;
            }}
            className="h-6 min-w-[90px] flex-1 bg-transparent text-sm outline-none placeholder:text-ink-faint"
          />
        </div>
        {open && (
          <ul
            role="listbox"
            className="absolute inset-x-0 top-full z-20 mt-1 overflow-hidden rounded-inner border border-line bg-elevated py-1 shadow-lg"
          >
            {suggestions.map((tag, i) => (
              <li key={tag} role="option" aria-selected={i === pick}>
                <button
                  // `mousedown`, not `click`: the click would come after the
                  // field's blur, and that already adds what was typed.
                  onMouseDown={(event) => {
                    event.preventDefault();
                    add(tag);
                  }}
                  onMouseEnter={() => setPick(i)}
                  className={cn(
                    "flex w-full items-center gap-1 px-3 py-1.5 text-left text-sm",
                    i === pick ? "bg-hover text-ink" : "text-ink-muted",
                  )}
                >
                  <span className="text-ink-faint">#</span>
                  {tag}
                </button>
              </li>
            ))}
          </ul>
        )}
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
