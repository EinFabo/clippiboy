import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useEngine } from "@/store";
import { Card, Pill } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { ClipPlayer } from "@/components/ClipPlayer";
import { useClipMenu } from "@/components/clipMenu";
import {
  IconCheck,
  IconClose,
  IconFolder,
  IconHeart,
  IconScissors,
  IconSearch,
  IconTrash,
} from "@/components/icons";
import type { Route } from "@/components/NavBar";
import { api, fileUrl, inTauri } from "@/lib/ipc";
import { clipName, fileName, formatAgo, formatDuration, formatSize } from "@/lib/format";
import { cn } from "@/lib/cn";
import { isTextField } from "@/lib/dom";
import type { Clip } from "@/lib/types";

/**
 * What the gallery is filtering by right now.
 *
 * A type of its own instead of a game name with magic values: "favorites" and
 * "no game" are not games, and a game that happened to be called that should not
 * throw the filter off.
 */
type Filter =
  | { kind: "all" }
  | { kind: "favorites" }
  | { kind: "untagged" }
  | { kind: "game"; name: string };

const ALL: Filter = { kind: "all" };

export function Clips({ onNavigate }: { onNavigate: (r: Route) => void }) {
  const { clips, deleteClip, clearGame, setFavorite, fileClip } = useEngine();
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<Filter>(ALL);
  /** Which clip is in the player. Editing always happens there. */
  const [open, setOpen] = useState<number | null>(null);
  /**
   * The playlist as it looked when the player opened — as ids.
   *
   * It must not change under the player: entering a clip's game in the editor
   * while the gallery filters by exactly that game would otherwise drop it out
   * of the list mid-typing, and the player would suddenly sit on a different
   * clip.
   */
  const [playlist, setPlaylist] = useState<string[]>([]);
  /**
   * Which clips were touched in the player. Their files move into the matching
   * folder on close — that would not work while it is open, since the player
   * holds the file.
   */
  const touched = useRef<Set<string>>(new Set());
  /** Which clip is being renamed — both the click on the name and "Rename" in
      the right-click menu land here. */
  const [renaming, setRenaming] = useState<string | null>(null);
  const clipMenu = useClipMenu();

  /** Games with a count, most frequent first — the bar should lead with what
      people actually filter by. */
  const games = useMemo(() => {
    const counts = new Map<string, number>();
    for (const clip of clips) {
      if (clip.game) counts.set(clip.game, (counts.get(clip.game) ?? 0) + 1);
    }
    return [...counts]
      .map(([name, count]) => ({ name, count }))
      .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name, "en"));
  }, [clips]);

  const untagged = useMemo(() => clips.filter((c) => !c.game).length, [clips]);
  const favorites = useMemo(() => clips.filter((c) => c.favorite).length, [clips]);

  // If a clip's game is renamed or removed, its filter disappears — without
  // this the gallery would stay empty and nobody would know why. Not while the
  // player is open, though: that is where the change is being made, and the
  // filter behind it should stay put.
  useEffect(() => {
    if (filter.kind === "all" || open !== null) return;
    const gone =
      filter.kind === "untagged"
        ? untagged === 0
        : filter.kind === "favorites"
          ? favorites === 0
          : !games.some((g) => g.name === filter.name);
    if (gone) setFilter(ALL);
  }, [filter, games, untagged, favorites, open]);

  const visible = clips.filter((c) => {
    const haystack = [
      fileName(c.path),
      c.title ?? "",
      c.description ?? "",
      c.game ?? "",
    ]
      .join(" ")
      .toLowerCase();
    const matchesQuery = !query || haystack.includes(query.toLowerCase());
    const matchesFilter =
      filter.kind === "all"
        ? true
        : filter.kind === "favorites"
          ? c.favorite
          : filter.kind === "untagged"
            ? !c.game
            : c.game === filter.name;
    return matchesQuery && matchesFilter;
  });

  const filtered = query !== "" || filter.kind !== "all";

  // Deleted clips fall out of the playlist, changed ones stay in it — with
  // whatever state the store currently holds.
  const playing = useMemo(() => {
    const byId = new Map(clips.map((clip) => [clip.id, clip]));
    return playlist
      .map((id) => byId.get(id))
      .filter((clip): clip is Clip => clip !== undefined);
  }, [playlist, clips]);

  /** Open the player with the list currently shown in the gallery. */
  const openAt = (index: number) => {
    setPlaylist(visible.map((clip) => clip.id));
    touched.current = new Set(visible[index] ? [visible[index].id] : []);
    setOpen(index);
  };

  /** On close, catch up on what was not possible during playback: move the
      files of the clips that were viewed into their folder. */
  const closePlayer = () => {
    setOpen(null);
    const seen = [...touched.current];
    touched.current = new Set();
    for (const id of seen) void fileClip(id);
  };

  /** Heart on or off — and take the file along right away. */
  const toggleFavorite = async (clip: Clip) => {
    await setFavorite(clip.id, !clip.favorite);
    await fileClip(clip.id);
  };

  return (
    <div className="space-y-6">
      <header className="pt-10">
        <h1 className="display text-4xl">Clips</h1>
      </header>

      <div className="space-y-3">
        <div className="flex items-center gap-3">
          <SearchField value={query} onChange={setQuery} />
          <span className="ml-auto shrink-0 text-xs text-ink-faint">
            {filtered
              ? `${visible.length} of ${clips.length} clips`
              : `${clips.length} clips`}
          </span>
        </div>

        <GameFilters
          games={games}
          untagged={untagged}
          favorites={favorites}
          total={clips.length}
          active={filter}
          onSelect={setFilter}
          onRemove={clearGame}
        />
      </div>

      {visible.length === 0 ? (
        <Card className="grid h-56 place-items-center text-sm text-ink-muted">
          No clips found.
        </Card>
      ) : (
        <div className="grid grid-cols-3 gap-4 pb-10">
          {visible.map((clip, index) => (
            <Card
              key={clip.id}
              interactive
              className="group overflow-hidden"
              onContextMenu={(event) => {
                // If the cursor is in the name field the menu belongs to the
                // text — `TextMenu` takes care of that on its own.
                if (isTextField(event.target)) return;
                clipMenu(event, clip, {
                  onOpen: () => openAt(index),
                  onRename: () => setRenaming(clip.id),
                  onDelete: () => deleteClip(clip.id),
                });
              }}
            >
              <div className="relative">
                <button
                  onClick={() => openAt(index)}
                  aria-label={`Play ${clip.game ?? "clip"}`}
                  className="relative block aspect-video w-full bg-gradient-to-br from-accent-deep/40 to-black"
                >
                  {clip.thumbPath && (
                    <img
                      // After a trim there is a new picture under the same path.
                      // Without the suffix the WebView would keep showing the one
                      // from its cache — a moment that no longer appears in the
                      // clip at all.
                      src={`${fileUrl(clip.thumbPath)}?v=${clip.sizeBytes}`}
                      alt=""
                      className="h-full w-full object-cover"
                    />
                  )}
                  {/* Play symbol on hover only — it covers the picture otherwise. */}
                  <span
                    className="absolute inset-0 grid place-items-center bg-black/30 opacity-0
                      transition-opacity duration-200 group-hover:opacity-100"
                  >
                    <span className="grid h-12 w-12 place-items-center rounded-pill bg-white/90 text-black">
                      <svg viewBox="0 0 24 24" className="h-5 w-5 translate-x-[1px]" fill="currentColor">
                        <path d="M7.5 5.2 19 12 7.5 18.8V5.2Z" />
                      </svg>
                    </span>
                  </span>
                  {/* Both pills on one row: a long game name would otherwise
                      slide under the duration instead of truncating. */}
                  <span className="absolute inset-x-3 bottom-3 flex items-end justify-between gap-2">
                    <Pill className="min-w-0">
                      <span className="min-w-0 truncate">{clip.game ?? "Unknown"}</span>
                    </Pill>
                    <span className="flex shrink-0 items-center gap-1.5">
                      {/* Only on a real trim: you cannot see a changed mix on a
                          clip, but you can see its length — and that the original
                          still sits beside it is the fact that counts here. */}
                      {clip.original && (
                        <Pill title="Trimmed — the original sits beside it">
                          <IconScissors className="h-3 w-3" />
                        </Pill>
                      )}
                      <Pill>{formatDuration(clip.durationMs)}</Pill>
                    </span>
                  </span>
                </button>
                {/* The heart stays visible once set — otherwise you would have
                    to hover every tile to see your favorites. */}
                <button
                  aria-label={
                    clip.favorite ? "Remove from favorites" : "Add to favorites"
                  }
                  aria-pressed={clip.favorite}
                  title={
                    clip.favorite
                      ? 'Favorite — the file lives in the "Favorites" folder'
                      : "Add to favorites"
                  }
                  onClick={() => void toggleFavorite(clip)}
                  className={cn(
                    "absolute top-3 left-3 grid h-8 w-8 place-items-center rounded-pill",
                    "bg-black/50 backdrop-blur-md transition",
                    clip.favorite
                      ? "text-live"
                      : "text-white/70 opacity-0 group-hover:opacity-100 hover:text-white",
                  )}
                >
                  <IconHeart filled={clip.favorite} className="h-4 w-4" />
                </button>
                <div
                  className="absolute top-3 right-3 flex gap-1.5 opacity-0 transition-opacity
                    group-hover:opacity-100"
                >
                  <IconAction
                    label="Show in folder"
                    onClick={() => inTauri && api.revealClip(clip.id)}
                  >
                    <IconFolder className="h-4 w-4" />
                  </IconAction>
                  <IconAction
                    label="Delete clip"
                    danger
                    onClick={() => deleteClip(clip.id)}
                  >
                    <IconTrash className="h-4 w-4" />
                  </IconAction>
                </div>
              </div>
              <div className="p-4">
                <NameField
                  clip={clip}
                  editing={renaming === clip.id}
                  onEditing={(on) => setRenaming(on ? clip.id : null)}
                />
                <p className="mt-1 truncate text-xs text-ink-muted">
                  {clip.game ? `${clip.game} · ` : ""}
                  {formatAgo(clip.createdAt)} · {clip.height}p ·{" "}
                  {formatSize(clip.sizeBytes)}
                </p>
                {clip.description && (
                  <p className="mt-1 truncate text-xs text-ink-faint">
                    {clip.description}
                  </p>
                )}
                {/* Only one button: viewing and editing have become the same
                    screen. */}
                <div className="mt-3">
                  <Button size="sm" onClick={() => openAt(index)}>
                    Open
                  </Button>
                </div>
              </div>
            </Card>
          ))}
        </div>
      )}

      {open !== null && playing.length > 0 && (
        <ClipPlayer
          clips={playing}
          index={Math.min(open, playing.length - 1)}
          onIndexChange={(next) => {
            const clip = playing[next];
            if (clip) touched.current.add(clip.id);
            setOpen(next);
          }}
          onClose={closePlayer}
          onDelete={deleteClip}
          onOpenMixer={() => onNavigate("audio")}
        />
      )}
    </div>
  );
}

/**
 * The name on the tile — one click turns it into a field.
 *
 * Renaming should not mean opening the player first. The name comes up selected,
 * Enter and a click elsewhere save, Escape discards. Only the name is saved; the
 * file in the folder keeps its own.
 */
function NameField({
  clip,
  editing,
  onEditing,
}: {
  clip: Clip;
  /** Driven from outside so "Rename" in the right-click menu lands here. */
  editing: boolean;
  onEditing: (on: boolean) => void;
}) {
  const updateClip = useEngine((state) => state.updateClip);
  const [draft, setDraft] = useState("");
  // Escape takes focus off the field, and that would otherwise still trigger the
  // save that was just cancelled.
  const cancelled = useRef(false);

  // The draft starts at the current name — whether the field opens via the click
  // or via the menu.
  useEffect(() => {
    if (editing) {
      cancelled.current = false;
      setDraft(clip.title ?? "");
    }
  }, [editing, clip.title]);

  const commit = () => {
    onEditing(false);
    if (cancelled.current) {
      cancelled.current = false;
      return;
    }
    const next = draft.trim();
    if (next === (clip.title ?? "")) return;
    void updateClip(clip.id, {
      title: next || null,
      description: clip.description,
      game: clip.game,
    });
  };

  if (!editing) {
    return (
      <button
        title="Click to rename"
        onClick={() => onEditing(true)}
        className="-mx-1.5 block w-[calc(100%+0.75rem)] truncate rounded-inner px-1.5 py-0.5
          text-left text-sm font-medium transition-colors hover:bg-hover"
      >
        {clipName(clip)}
      </button>
    );
  }

  return (
    <input
      autoFocus
      value={draft}
      aria-label="Clip name"
      placeholder={fileName(clip.path)}
      onFocus={(e) => e.currentTarget.select()}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") e.currentTarget.blur();
        if (e.key === "Escape") {
          cancelled.current = true;
          e.currentTarget.blur();
        }
      }}
      className="-mx-1.5 w-[calc(100%+0.75rem)] rounded-inner border border-line-strong
        bg-elevated px-1.5 py-0.5 text-sm font-medium outline-none
        placeholder:font-normal placeholder:text-ink-faint"
    />
  );
}

function SearchField({
  value,
  onChange,
}: {
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <div className="relative w-80">
      <IconSearch className="pointer-events-none absolute top-1/2 left-4 h-4 w-4 -translate-y-1/2 text-ink-faint" />
      <input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => e.key === "Escape" && onChange("")}
        placeholder="Search…"
        className="h-10 w-full rounded-pill border border-line bg-surface pr-10 pl-10 text-sm
          outline-none transition-colors placeholder:text-ink-faint focus:border-line-strong"
      />
      {value && (
        <button
          aria-label="Clear search"
          onClick={() => onChange("")}
          className="absolute top-1/2 right-3 grid h-6 w-6 -translate-y-1/2 place-items-center
            rounded-pill text-ink-faint transition-colors hover:bg-hover hover:text-ink"
        >
          <IconClose className="h-3.5 w-3.5" />
        </button>
      )}
    </div>
  );
}

/**
 * The game filters as a single horizontally scrolling row.
 *
 * The bar used to grow out of the window to the right, and long window titles
 * wrapped inside their pill. Now: one row, fixed height, long names truncate —
 * and whatever is not a game at all can be cleared away with the ×, instead of
 * standing there forever.
 */
function GameFilters({
  games,
  untagged,
  favorites,
  total,
  active,
  onSelect,
  onRemove,
}: {
  games: Array<{ name: string; count: number }>;
  untagged: number;
  favorites: number;
  total: number;
  active: Filter;
  onSelect: (filter: Filter) => void;
  onRemove: (game: string) => void;
}) {
  const strip = useRef<HTMLDivElement>(null);
  const [fade, setFade] = useState({ left: false, right: false });
  const [confirming, setConfirming] = useState<string | null>(null);

  const measure = useCallback(() => {
    const el = strip.current;
    if (!el) return;
    const max = el.scrollWidth - el.clientWidth;
    setFade({ left: el.scrollLeft > 2, right: el.scrollLeft < max - 2 });
  }, []);

  useLayoutEffect(measure, [measure, games, untagged, favorites]);

  // Tip the mouse wheel over: there is nothing in the bar that could scroll
  // vertically, so the wheel should move it horizontally. Only when it really
  // overflows — otherwise it would take scrolling away from the gallery for no
  // reason.
  useEffect(() => {
    const el = strip.current;
    if (!el) return;
    const onWheel = (event: WheelEvent) => {
      if (event.deltaY === 0 || el.scrollWidth <= el.clientWidth) return;
      event.preventDefault();
      el.scrollLeft += event.deltaY;
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => {
      el.removeEventListener("wheel", onWheel);
      observer.disconnect();
    };
  }, [measure]);

  // The confirmation must not stay up when you carry on elsewhere.
  useEffect(() => {
    if (!confirming) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setConfirming(null);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [confirming]);

  const edge = (on: boolean) => (on ? "36px" : "0px");
  const mask = `linear-gradient(to right, transparent, #000 ${edge(fade.left)},
    #000 calc(100% - ${edge(fade.right)}), transparent)`;

  return (
    <div
      ref={strip}
      onScroll={measure}
      className="no-scrollbar flex items-center gap-2 overflow-x-auto py-0.5"
      style={{ maskImage: mask, WebkitMaskImage: mask }}
    >
      <Chip
        active={active.kind === "all"}
        onClick={() => onSelect({ kind: "all" })}
        count={total}
      >
        All
      </Chip>

      {favorites > 0 && (
        <Chip
          active={active.kind === "favorites"}
          count={favorites}
          onClick={() => onSelect({ kind: "favorites" })}
          icon={
            <IconHeart
              filled
              className={cn(
                "h-3.5 w-3.5",
                active.kind === "favorites" ? "text-black/70" : "text-live",
              )}
            />
          }
        >
          Favorites
        </Chip>
      )}

      {games.map(({ name, count }) =>
        confirming === name ? (
          <ConfirmChip
            key={name}
            name={name}
            onConfirm={() => {
              onRemove(name);
              setConfirming(null);
            }}
            onCancel={() => setConfirming(null)}
          />
        ) : (
          <Chip
            key={name}
            active={active.kind === "game" && active.name === name}
            count={count}
            onClick={() => onSelect({ kind: "game", name })}
            onRemove={() => setConfirming(name)}
          >
            {name}
          </Chip>
        ),
      )}

      {untagged > 0 && (
        <Chip
          active={active.kind === "untagged"}
          count={untagged}
          onClick={() => onSelect({ kind: "untagged" })}
        >
          No game
        </Chip>
      )}
    </div>
  );
}

/**
 * One filter pill. The × button holds its place in the layout and only becomes
 * visible on hover — if it did not, every following pill would jump sideways as
 * you moved across.
 */
function Chip({
  active,
  count,
  icon,
  onClick,
  onRemove,
  children,
}: {
  active: boolean;
  count: number;
  icon?: React.ReactNode;
  onClick: () => void;
  onRemove?: () => void;
  children: string;
}) {
  return (
    <div
      className={cn(
        "group/chip flex h-8 shrink-0 items-center rounded-pill border",
        "transition-colors duration-150",
        active
          ? "border-white bg-white text-black"
          : "border-line bg-surface text-ink-muted hover:border-line-strong hover:text-ink",
      )}
    >
      <button
        onClick={onClick}
        title={children}
        className={cn(
          "flex h-full min-w-0 items-center gap-1.5 rounded-pill pl-4 text-[13px] font-medium",
          onRemove ? "pr-1.5" : "pr-4",
        )}
      >
        {icon}
        <span className="max-w-[180px] truncate">{children}</span>
        <span className={cn("tabular-nums", active ? "text-black/45" : "text-ink-faint")}>
          {count}
        </span>
      </button>
      {onRemove && (
        <button
          aria-label={`Remove the "${children}" filter`}
          title="Remove filter — the game name is detached from these clips"
          onClick={onRemove}
          className={cn(
            "mr-1 grid h-6 w-6 shrink-0 place-items-center rounded-pill opacity-0 transition",
            "group-hover/chip:opacity-100 focus-visible:opacity-100",
            active ? "hover:bg-black/10 hover:text-black" : "hover:bg-hover hover:text-live",
          )}
        >
          <IconClose className="h-3 w-3" />
        </button>
      )}
    </div>
  );
}

/** The confirmation sits in the pill's place — no dialog over the page. */
function ConfirmChip({
  name,
  onConfirm,
  onCancel,
}: {
  name: string;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <div
      className="flex h-8 shrink-0 items-center gap-1 rounded-pill border border-live/40
        bg-live/15 pl-4 text-[13px] font-medium text-live"
    >
      <span className="max-w-[160px] truncate" title={name}>
        {name}
      </span>
      <span className="whitespace-nowrap">remove?</span>
      <button
        aria-label="Confirm removal"
        onClick={onConfirm}
        autoFocus
        className="ml-1 grid h-6 w-6 place-items-center rounded-pill hover:bg-live/25"
      >
        <IconCheck className="h-3.5 w-3.5" />
      </button>
      <button
        aria-label="Cancel"
        onClick={onCancel}
        className="mr-1 grid h-6 w-6 place-items-center rounded-pill text-ink-muted hover:bg-hover hover:text-ink"
      >
        <IconClose className="h-3 w-3" />
      </button>
    </div>
  );
}

/** Round button over the thumbnail. */
function IconAction({
  label,
  danger,
  onClick,
  children,
}: {
  label: string;
  danger?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      aria-label={label}
      title={label}
      onClick={onClick}
      className={cn(
        "grid h-8 w-8 place-items-center rounded-pill bg-black/50 text-white/70",
        "backdrop-blur-md transition-colors",
        danger ? "hover:text-live" : "hover:text-white",
      )}
    >
      {children}
    </button>
  );
}
