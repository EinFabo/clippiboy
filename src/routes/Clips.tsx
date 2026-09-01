import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useEngine } from "@/store";
import { Card, Pill } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { ClipPlayer } from "@/components/ClipPlayer";
import { ShotViewer } from "@/components/ShotViewer";
import { useClipMenu } from "@/components/clipMenu";
import {
  IconCamera,
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
import { useFlip } from "@/lib/useFlip";
import { usePresence } from "@/lib/usePresence";
import { useCountUp } from "@/lib/useCountUp";
import { HeartBurst } from "@/components/ui/HeartBurst";
import { ConfirmDelete } from "@/components/ui/ConfirmDelete";
import { ExportDialog } from "@/components/ExportDialog";
import { animate, EASE_SPRING } from "@/lib/motion";
import type { Clip } from "@/lib/types";

/**
 * What the gallery is filtering by right now.
 *
 * A type of its own instead of a game name with magic values: "favorites" and
 * "no game" are not games, and a game that happened to be called that should not
 * throw the filter off.
 */
type Filter =
  /** Recordings. Stills have a chip of their own — see `screenshots`. */
  | { kind: "all" }
  | { kind: "favorites" }
  | { kind: "screenshots" }
  | { kind: "untagged" }
  | { kind: "game"; name: string };

const ALL: Filter = { kind: "all" };

/** Has to match the length of `cb-tile-out` in styles/motion.css. */
const LEAVE_MS = 200;

/**
 * Whether the gallery has already introduced itself in this session.
 *
 * The staggered arrival is a greeting, not a habit: coming back from another
 * tab for the fourth time should simply show the clips.
 */
let greeted = false;

export function Clips({ onNavigate }: { onNavigate: (r: Route) => void }) {
  const { clips, deleteClip, discardClipOriginal, clearGame, setFavorite, fileClip } =
    useEngine();
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
  /**
   * Which clip has been asked about. Deleting takes the file off the disk and
   * there is no way back, so the bin does not delete — it asks, in its own
   * place on the tile.
   */
  const [confirming, setConfirming] = useState<string | null>(null);
  // Throwing the untouched recording away cannot be undone either, so it asks
  // in the same place and the same way as deleting does.
  const [discarding, setDiscarding] = useState<string | null>(null);
  // The clip whose export dialog is open, if any.
  const [exporting, setExporting] = useState<Clip | null>(null);
  const clipMenu = useClipMenu();
  const grid = useRef<HTMLDivElement>(null);
  /**
   * The thumbnails by clip id. The player asks for one when it opens and again
   * when it closes, so the picture has somewhere to grow out of and back into.
   */
  const thumbs = useRef(new Map<string, HTMLElement>());
  const originOf = useCallback(
    (id: string) => thumbs.current.get(id)?.getBoundingClientRect() ?? null,
    [],
  );

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
  const shots = useMemo(() => clips.filter((c) => c.screenshot).length, [clips]);
  const recordings = clips.length - shots;

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
          : filter.kind === "screenshots"
            ? shots === 0
            : !games.some((g) => g.name === filter.name);
    if (gone) setFilter(ALL);
  }, [filter, games, untagged, favorites, shots, open]);

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
        ? !c.screenshot
        : filter.kind === "favorites"
          ? c.favorite
          : filter.kind === "screenshots"
            ? c.screenshot
            : filter.kind === "untagged"
              ? !c.game
              : c.game === filter.name;
    return matchesQuery && matchesFilter;
  });

  const filtered = query !== "" || filter.kind !== "all";

  const clipIds = useMemo(() => new Set(clips.map((c) => c.id)), [clips]);

  /**
   * The tiles, including the ones on their way out. Only a clip that was really
   * deleted is held back for its exit — one that merely fell out of the filter
   * goes at once, or every keystroke in the search field would drag a trail of
   * ghosts behind it.
   */
  const tiles = usePresence(visible, (clip) => clip.id, LEAVE_MS, (id) => !clipIds.has(id));

  /** Where a tile stands in the live list — the player counts in those. */
  const positions = new Map(visible.map((clip, index) => [clip.id, index]));

  // Deleting one tile and filtering the rest are the same thing to the grid:
  // whatever stays has to travel to its new place instead of appearing there.
  useFlip(grid, tiles.map((tile) => (tile.leaving ? `${tile.key}!` : tile.key)).join());

  // Which tiles arrive with an animation, decided once and then left alone —
  // recomputing it every render would cut the animation off halfway.
  const greeting = useRef<Set<string> | null>(null);
  const known = useRef<Set<string> | null>(null);
  const arrived = useRef(new Set<string>());

  if (greeting.current === null && tiles.length > 0) {
    greeting.current = greeted ? new Set() : new Set(tiles.map((tile) => tile.key));
    greeted = true;
  }
  if (known.current === null) {
    known.current = new Set(clips.map((clip) => clip.id));
  } else {
    // A clip saved while the gallery is open comes in at the front
    // (store.ts:163) and should be seen doing it.
    for (const clip of clips) {
      if (known.current.has(clip.id)) continue;
      known.current.add(clip.id);
      arrived.current.add(clip.id);
    }
  }

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

  /** Page on inside the open viewer. Both of them do it the same way. */
  const openIndex = (next: number) => {
    const clip = playing[next];
    if (clip) touched.current.add(clip.id);
    setOpen(next);
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
          <span className="ml-auto shrink-0 text-xs text-ink-faint tabular-nums">
            {filtered ? (
              <>
                <Counted value={visible.length} /> of {clips.length} clips
              </>
            ) : (
              <>
                <Counted value={clips.length} /> clips
              </>
            )}
          </span>
        </div>

        <GameFilters
          games={games}
          untagged={untagged}
          favorites={favorites}
          shots={shots}
          total={recordings}
          active={filter}
          onSelect={setFilter}
          onRemove={clearGame}
        />
      </div>

      {tiles.length === 0 ? (
        <Card className="grid h-56 place-items-center text-sm text-ink-muted">
          No clips found.
        </Card>
      ) : (
        <div ref={grid} className="grid grid-cols-3 gap-4 pb-10">
          {tiles.map(({ item: clip, key, leaving }, slot) => {
            const index = positions.get(clip.id) ?? 0;
            // Capped, so a gallery of eighty clips does not crawl in.
            const wait = greeting.current?.has(key) ? Math.min(slot, 11) * 30 : null;
            return (
            <Card
              key={key}
              data-flip={key}
              interactive={!leaving}
              className={cn(
                "group overflow-hidden",
                leaving
                  ? "cb-tile-out"
                  : (wait !== null || arrived.current.has(key)) && "cb-tile-in",
              )}
              style={wait !== null ? { animationDelay: `${wait}ms` } : undefined}
              onContextMenu={(event) => {
                // If the cursor is in the name field the menu belongs to the
                // text — `TextMenu` takes care of that on its own.
                if (isTextField(event.target)) return;
                clipMenu(event, clip, {
                  onOpen: () => openAt(index),
                  onRename: () => setRenaming(clip.id),
                  onDelete: () => setConfirming(clip.id),
                  onDiscardOriginal: () => setDiscarding(clip.id),
                  onExport: () => setExporting(clip),
                });
              }}
            >
              <div className="relative">
                <button
                  ref={(node) => {
                    if (node) thumbs.current.set(clip.id, node);
                    else thumbs.current.delete(clip.id);
                  }}
                  onClick={() => openAt(index)}
                  aria-label={`${clip.screenshot ? "Open" : "Play"} ${
                    clip.game ?? (clip.screenshot ? "screenshot" : "clip")
                  }`}
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
                  {/* Sign on hover only — it covers the picture otherwise. And
                      no play symbol over a still: there is nothing to play. */}
                  <span
                    className="absolute inset-0 grid place-items-center bg-black/30 opacity-0
                      transition-opacity duration-200 group-hover:opacity-100"
                  >
                    <span className="grid h-12 w-12 place-items-center rounded-pill bg-white/90 text-black">
                      {clip.screenshot ? (
                        <IconCamera className="h-5 w-5" />
                      ) : (
                        <svg viewBox="0 0 24 24" className="h-5 w-5 translate-x-[1px]" fill="currentColor">
                          <path d="M7.5 5.2 19 12 7.5 18.8V5.2Z" />
                        </svg>
                      )}
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
                      {/* A still has no length. The camera says what the
                          duration would have said on a clip. */}
                      {clip.screenshot ? (
                        <Pill title="Screenshot">
                          <IconCamera className="h-3 w-3" />
                        </Pill>
                      ) : (
                        <Pill>{formatDuration(clip.durationMs)}</Pill>
                      )}
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
                  <HeartBurst favorite={clip.favorite} />
                </button>
                <div
                  className={cn(
                    "absolute top-3 right-3 flex gap-1.5 transition-opacity",
                    // While the question stands it has to stay up, even once
                    // the cursor has wandered off the tile.
                    confirming === clip.id || discarding === clip.id
                      ? "opacity-100"
                      : "opacity-0 group-hover:opacity-100",
                  )}
                >
                  {confirming === clip.id ? (
                    <ConfirmDelete
                      origin="right"
                      onConfirm={() => {
                        setConfirming(null);
                        void deleteClip(clip.id);
                      }}
                      onCancel={() => setConfirming(null)}
                    />
                  ) : discarding === clip.id ? (
                    <ConfirmDelete
                      origin="right"
                      question="Throw the recording away?"
                      confirmLabel="Throw away"
                      confirmTitle="The trim stays, undo goes"
                      onConfirm={() => {
                        setDiscarding(null);
                        void discardClipOriginal(clip.id);
                      }}
                      onCancel={() => setDiscarding(null)}
                    />
                  ) : (
                    <>
                      <IconAction
                        label="Show in folder"
                        onClick={() => inTauri && api.revealClip(clip.id)}
                      >
                        <IconFolder className="h-4 w-4" />
                      </IconAction>
                      <IconAction
                        label="Delete clip"
                        danger
                        onClick={() => setConfirming(clip.id)}
                      >
                        <IconTrash className="h-4 w-4" />
                      </IconAction>
                    </>
                  )}
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
                  {formatAgo(clip.createdAt)} ·{" "}
                  {/* A still is measured by its edges, not by "1080p" — that is
                      a word about video. */}
                  {clip.screenshot
                    ? `${clip.width} × ${clip.height}`
                    : `${clip.height}p`}{" "}
                  · {formatSize(clip.sizeBytes)}
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
            );
          })}
        </div>
      )}

      {/* Which of the two opens is decided per entry, not per playlist: paging
          through a gallery of both leads from a clip into a picture and back. */}
      {open !== null && playing.length > 0 && (
        playing[Math.min(open, playing.length - 1)]?.screenshot ? (
          <ShotViewer
            clips={playing}
            index={Math.min(open, playing.length - 1)}
            onIndexChange={openIndex}
            onClose={closePlayer}
            onDelete={deleteClip}
            originOf={originOf}
          />
        ) : (
          <ClipPlayer
            clips={playing}
            index={Math.min(open, playing.length - 1)}
            onIndexChange={openIndex}
            onClose={closePlayer}
            onDelete={deleteClip}
            onOpenMixer={() => onNavigate("audio")}
            originOf={originOf}
          />
        )
      )}

      {exporting && (
        <ExportDialog clip={exporting} onClose={() => setExporting(null)} />
      )}
    </div>
  );
}

/** A count that runs to its new reading instead of flicking to it. */
function Counted({ value }: { value: number }) {
  return <>{Math.round(useCountUp(value))}</>;
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
  shots,
  total,
  active,
  onSelect,
  onRemove,
}: {
  games: Array<{ name: string; count: number }>;
  untagged: number;
  favorites: number;
  shots: number;
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

  useLayoutEffect(measure, [measure, games, untagged, favorites, shots]);

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

  // Chips are sorted by how often a game turns up, and the confirmation takes
  // the place of the chip it belongs to. Both shift everything to the right of
  // them, and that should be a slide rather than a jump.
  useFlip(
    strip,
    [
      "all",
      favorites > 0 && "favorites",
      shots > 0 && "screenshots",
      ...games.map(({ name }) => (confirming === name ? `${name}?` : name)),
      untagged > 0 && "untagged",
    ]
      .filter(Boolean)
      .join(),
  );

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
      {/* Was called "All" while there was only one kind of thing in here.
          Now it names what it shows, and the stills sit beside it. */}
      <Chip
        id="all"
        active={active.kind === "all"}
        onClick={() => onSelect({ kind: "all" })}
        count={total}
      >
        Clips
      </Chip>

      {favorites > 0 && (
        <Chip
          id="favorites"
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

      {shots > 0 && (
        <Chip
          id="screenshots"
          active={active.kind === "screenshots"}
          count={shots}
          onClick={() => onSelect({ kind: "screenshots" })}
          icon={
            <IconCamera
              className={cn(
                "h-3.5 w-3.5",
                active.kind === "screenshots" ? "text-black/70" : "text-ink-muted",
              )}
            />
          }
        >
          Screenshots
        </Chip>
      )}

      {games.map(({ name, count }) =>
        confirming === name ? (
          <ConfirmChip
            key={name}
            id={name}
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
            id={name}
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
          id="untagged"
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
  id,
  active,
  count,
  icon,
  onClick,
  onRemove,
  children,
}: {
  id: string;
  active: boolean;
  count: number;
  icon?: React.ReactNode;
  onClick: () => void;
  onRemove?: () => void;
  children: string;
}) {
  const box = useRef<HTMLDivElement>(null);
  const was = useRef(active);

  // A single small nod when the chip takes over — not on mount, or the whole
  // bar would twitch every time the gallery opens.
  useEffect(() => {
    if (was.current === active) return;
    was.current = active;
    if (!active) return;
    animate(
      box.current,
      [
        { transform: "scale(1)" },
        { transform: "scale(1.04)", offset: 0.45 },
        { transform: "scale(1)" },
      ],
      { duration: 300, easing: EASE_SPRING },
    );
  }, [active]);

  return (
    <div
      ref={box}
      data-flip={id}
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
  id,
  name,
  onConfirm,
  onCancel,
}: {
  id: string;
  name: string;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <div
      data-flip={id}
      className="cb-chip-expand flex h-8 shrink-0 items-center gap-1 rounded-pill border
        border-live/40 bg-live/15 pl-4 text-[13px] font-medium text-live"
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
