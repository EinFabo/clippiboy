import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { create } from "zustand";
import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card, Pill } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Segmented } from "@/components/ui/Controls";
import { useMenu } from "@/components/ui/Menu";
import { FacetPopover } from "@/components/FacetPopover";
import { ClipPlayer } from "@/components/ClipPlayer";
import { ShotViewer } from "@/components/ShotViewer";
import { ShareMenuButton, useClipMenu } from "@/components/clipMenu";
import { timeLeft, useLinks, type Link } from "@/lib/links";
import {
  IconCamera,
  IconCheck,
  IconClose,
  IconFolder,
  IconHeart,
  IconLink,
  IconRecord,
  IconScissors,
  IconSearch,
  IconSend,
  IconTrash,
} from "@/components/icons";
import type { Route } from "@/components/NavBar";
import { api, fileUrl, inTauri } from "@/lib/ipc";
import { clipName, fileName, formatAgo, formatDuration, formatSize } from "@/lib/format";
import { cn } from "@/lib/cn";
import { hotkeyLabel, shownHotkey } from "@/lib/hotkeyText";
import { isTextField } from "@/lib/dom";
import { useFlip } from "@/lib/useFlip";
import { useStableHandler } from "@/lib/useStableHandler";
import { sameData } from "@/lib/same";
import { limitGroups } from "@/lib/limitGroups";
import { usePresence } from "@/lib/usePresence";
import { useCountUp } from "@/lib/useCountUp";
import { useClipSelection, type SelectMods } from "@/lib/useClipSelection";
import { moveInGrid, NAV_KEYS, type NavKey } from "@/lib/gridNav";
import { favoriteTarget } from "@/lib/clipSelect";
import { toast } from "@/lib/toastBus";
import { useFriends } from "@/lib/friends";
import { useShare } from "@/lib/share";
import { whenViewerFree } from "@/lib/viewerGuard";
import {
  NO_FILTER,
  facetLabel,
  filterClips,
  gameFacet,
  groupByDay,
  isFiltered,
  kindCounts,
  kindOf,
  pruneFilter,
  sortClips,
  tagFacet,
  toggled,
  type ClipFilter,
  type ClipKind,
  type ClipSort,
} from "@/lib/clipFilter";
import { HeartBurst } from "@/components/ui/HeartBurst";
import { ConfirmDelete } from "@/components/ui/ConfirmDelete";
import { ExportDialog } from "@/components/ExportDialog";
import { animate, EASE_SPRING } from "@/lib/motion";
import type { Clip, FocusClip, Trashed } from "@/lib/types";

/**
 * What the gallery shows: the filters, the sort, and whether Recently deleted
 * is open instead.
 *
 * Outside the component, so filters and sort outlive a trip to another page —
 * the routes unmount on every switch. Recently deleted is closed on the way
 * out. Never written anywhere, so a restart starts afresh.
 */
const useGallery = create<{ filter: ClipFilter; sort: ClipSort; inTrash: boolean }>(() => ({
  filter: NO_FILTER,
  sort: "newest",
  inTrash: false,
}));
const setFilter = (filter: ClipFilter) => useGallery.setState({ filter, inTrash: false });

const SORTS: Array<{ key: ClipSort; label: string }> = [
  { key: "newest", label: "Newest" },
  { key: "oldest", label: "Oldest" },
  { key: "longest", label: "Longest" },
  { key: "largest", label: "Largest" },
];

/** Has to match the length of `cb-tile-out` in styles/motion.css. */
const LEAVE_MS = 200;
/** Up to this many clips deleted together still leave with an animation. */
const BULK_ANIMATED = 12;
/** The gallery's columns — the arrow keys count rows in them. */
const GRID_COLUMNS = 3;

/**
 * Makes the grid one tab stop: every control of every tile but the resting one
 * leaves the tab order. Only what this sets is taken back, so a control with a
 * `tabIndex` of its own is left alone.
 */
function applyRoving(grid: HTMLElement, resting: string | null) {
  for (const tile of grid.querySelectorAll<HTMLElement>("[data-flip]")) roveTile(tile, resting);
}

/** {@link applyRoving} for one tile. */
function roveTile(tile: HTMLElement, resting: string | null) {
  const controls = "button, a[href], input, select, textarea, [tabindex], [data-roving]";
  const here = tile.dataset.flip === resting;
  for (const control of tile.querySelectorAll<HTMLElement>(controls)) {
    if (here) {
      if (control.dataset.roving) {
        control.removeAttribute("tabindex");
        delete control.dataset.roving;
      }
    } else if (!control.hasAttribute("tabindex")) {
      control.tabIndex = -1;
      control.dataset.roving = "1";
    }
  }
}

/** The tiles a batch of DOM changes touched: the ones changed inside, and the
 *  ones that arrived whole. */
function touchedTiles(records: MutationRecord[]): Set<HTMLElement> {
  const tiles = new Set<HTMLElement>();
  for (const record of records) {
    const inside = record.target instanceof Element ? record.target.closest<HTMLElement>("[data-flip]") : null;
    if (inside) {
      tiles.add(inside);
      continue;
    }
    for (const node of record.addedNodes) {
      if (!(node instanceof HTMLElement)) continue;
      if (node.matches("[data-flip]")) tiles.add(node);
      for (const tile of node.querySelectorAll<HTMLElement>("[data-flip]")) tiles.add(tile);
    }
  }
  return tiles;
}

/**
 * Whether the gallery has already introduced itself in this session.
 *
 * The staggered arrival is a greeting, not a habit: coming back from another
 * tab for the fourth time should simply show the clips.
 */
let greeted = false;
/** Tiles built on the first frame, the batch added per frame after it, and how
    many of the first ones play the entrance (about the first screen). */
const FIRST_TILES = 24;
const TILE_BATCH = 48;
const GREETED_TILES = 12;

export function Clips({
  onNavigate,
  focus,
  onFocused,
}: {
  onNavigate: (r: Route) => void;
  /** A clip to open straight away — the console over the game sends one when
   *  "open in the app" is pressed, with the second it stood at. */
  focus?: FocusClip | null;
  onFocused?: () => void;
}) {
  // `useShallow`, siehe `SourceTrouble`: ohne Selektor rendert die ganze
  // Galerie — über hundert Kacheln — zwanzigmal die Sekunde neu, weil die
  // Audiopegel durch denselben Store laufen. Das war der teuerste Posten.
  const bakeClipCam = useEngine((s) => s.bakeClipCam);
  const { clips, trash, deleteClip, deleteClips, discardClipOriginal, clearGame, setFavorite, fileClip } =
    useEngine(
      useShallow((s) => ({
        clips: s.clips,
        trash: s.trash,
        deleteClip: s.deleteClip,
        deleteClips: s.deleteClips,
        discardClipOriginal: s.discardClipOriginal,
        clearGame: s.clearGame,
        setFavorite: s.setFavorite,
        fileClip: s.fileClip,
      })),
    );
  const { filter, sort, inTrash } = useGallery();
  // Filters and sort wait for the way back; Recently deleted does not —
  // coming back to Clips shows the library.
  useEffect(() => () => useGallery.setState({ inTrash: false }), []);
  /** Which clip is in the player. Editing always happens there. */
  const [open, setOpen] = useState<number | null>(null);
  /** Sekunde, an der ein aus der Konsole übergebener Clip anfangen soll. Gilt
   *  nur für dieses eine Öffnen — jedes andere fängt vorne an. */
  const [startAt, setStartAt] = useState<number | undefined>(undefined);
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
  /** The clip on screen in the player, by id — see the effect after `closePlayer`. */
  const shownId = useRef<string | null>(null);
  /** A clip the player is about to show, until list and position have caught up. */
  const coming = useRef<string | null>(null);
  /** Clips this window's own player deleted. */
  const deletedHere = useRef<Set<string>>(new Set());
  /** Which clip is being renamed — both the click on the name and "Rename" in
      the right-click menu land here. */
  const [renaming, setRenaming] = useState<string | null>(null);
  /**
   * Which clip has been asked about, and what. Deleting takes the file off the
   * disk and there is no way back, so the bin does not delete — it asks, in its
   * own place on the tile. Throwing the untouched recording away cannot be
   * undone either, so it asks in the same place and the same way.
   */
  const [asking, setAsking] = useState<{ id: string; what: Asking } | null>(null);
  /** Clips with a live share link — a pill on the tile says so. */
  const links = useLinks((s) => s.links);
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

  /** Every game and tag in the library, whatever the type — what a choice may
      still point at. */
  const { values: games, noGame: untagged } = useMemo(() => gameFacet(clips, "all", []), [clips]);
  const tags = useMemo(() => tagFacet(clips, "all", []), [clips]);
  const favorites = useMemo(() => clips.filter((c) => c.favorite).length, [clips]);
  const kinds = useMemo(() => kindCounts(clips), [clips]);

  // If a clip's game is renamed or removed, its row disappears and so must
  // the choice — without this the gallery would stay empty and nobody would
  // know why. Not while the player is open, though: that is where the change
  // is being made, and the filter behind it should stay put.
  useEffect(() => {
    if (open !== null) return;
    const next = pruneFilter(filter, {
      games: games.map((g) => g.name),
      tags: tags.map((t) => t.name),
      favorites,
      untagged,
    });
    if (next !== filter) useGallery.setState({ filter: next });
    if (inTrash && trash.length === 0) useGallery.setState({ inTrash: false });
  }, [filter, games, tags, untagged, favorites, open, inTrash, trash.length]);

  /** The gallery in the order it is shown — the player pages through the same. */
  const visible = useMemo(
    () => (inTrash ? [] : sortClips(filterClips(clips, filter), sort)),
    [clips, filter, sort, inTrash],
  );

  const filtered = isFiltered(filter);

  const order = useMemo(() => visible.map((clip) => clip.id), [visible]);
  const selection = useClipSelection(order);
  const selecting = selection.count > 0;
  // The clip whose export dialog is open, if any.
  const [exporting, setExporting] = useState<Clip | null>(null);
  const picking = useShare((s) => s.picking);

  // Ctrl+A and Escape belong to the gallery only while nothing else is up: not
  // in a text field, not over the player, a dialog, the friend picker or a
  // question of its own.
  const confirmingSelection = useRef(false);
  useEffect(() => {
    if (open !== null || exporting || picking || inTrash) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || isTextField(event.target)) return;
      if (event.key === "Escape") {
        if (!confirmingSelection.current) selection.clear();
      } else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "a") {
        const target = event.target;
        // Only with the focus in the grid — the page itself is not "the gallery".
        if (!(target instanceof Node) || !grid.current?.contains(target)) return;
        event.preventDefault();
        selection.all();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, exporting, picking, inTrash, selection]);

  const clipIds = useMemo(() => new Set(clips.map((c) => c.id)), [clips]);

  /**
   * The tiles, including the ones on their way out. Only a clip that was really
   * deleted is held back for its exit — one that merely fell out of the filter
   * goes at once, or every keystroke in the search field would drag a trail of
   * ghosts behind it.
   */
  // A bulk delete of many goes without exits and without the others
  // travelling: a fading ghost and an animation per tile cost more there than
  // they show.
  const bulk = useRef(false);
  const tiles = usePresence(
    visible,
    (clip) => clip.id,
    LEAVE_MS,
    (id) => !bulk.current && !clipIds.has(id),
  );

  /** The tiles under their day headings — one group without a heading for the
      sorts that have no days. */
  const groups = groupByDay(tiles, (tile) => tile.item.createdAt, sort, Date.now());

  // The page opens with the first screens' worth of tiles and the rest follows
  // in the next frames, so the click is answered before 300 tiles are built.
  // Once everything is in, it stays in.
  const [built, setBuilt] = useState(FIRST_TILES);
  useEffect(() => {
    if (built === Infinity) return;
    const frame = requestAnimationFrame(() => setBuilt((n) => (n + TILE_BATCH >= tiles.length ? Infinity : n + TILE_BATCH)));
    return () => cancelAnimationFrame(frame);
  }, [built, tiles.length]);
  const shownGroups = limitGroups(groups, built);

  // The grid is one tab stop: only the tile the focus last rested on is
  // reachable with Tab, the arrow keys move between the tiles.
  const [resting, setResting] = useState<string | null>(null);
  const restingId = resting !== null && order.includes(resting) ? resting : (order[0] ?? null);
  const restingRef = useRef(restingId);
  restingRef.current = restingId;
  /** Which tiles stand where, the leaving ones marked — changes only when one
      comes, goes or moves. */
  const tileOrder = tiles.map((tile) => (tile.leaving ? `${tile.key}!` : tile.key)).join();
  const gridShown = !inTrash && tiles.length > 0;
  useLayoutEffect(() => {
    const node = grid.current;
    if (!node) return;
    applyRoving(node, restingId);
    // Tools that appear later (a question, the selection) are tiles' own
    // buttons again — only the tiles that changed are looked at again.
    const watch = new MutationObserver((records) => {
      for (const tile of touchedTiles(records)) roveTile(tile, restingId);
    });
    watch.observe(node, { childList: true, subtree: true });
    return () => watch.disconnect();
  }, [restingId, tileOrder, gridShown]);
  const onGridKeyDown = (event: React.KeyboardEvent) => {
    const target = event.target as HTMLElement;
    if (!target.matches("[data-tile-open]")) return;
    const id = target.closest<HTMLElement>("[data-flip]")?.dataset.flip;
    if (!id) return;
    if (event.key === " ") {
      if (event.repeat) return;
      // Space picks the tile; the button's own click would open it.
      event.preventDefault();
      selection.select(id, { shift: event.shiftKey });
      return;
    }
    if (!NAV_KEYS.includes(event.key) || event.ctrlKey || event.metaKey || event.altKey) return;
    const next = moveInGrid(
      groups.map((group) => group.items.filter((tile) => !tile.leaving).map((tile) => tile.item.id)),
      GRID_COLUMNS,
      id,
      event.key as NavKey,
    );
    event.preventDefault();
    if (next) thumbs.current.get(next)?.focus();
  };

  /** Where a tile stands in the live list — the player counts in those. */
  const positions = new Map(visible.map((clip, index) => [clip.id, index]));

  // Deleting one tile and filtering the rest are the same thing to the grid:
  // whatever stays has to travel to its new place instead of appearing there.
  useFlip(grid, tileOrder, undefined, bulk);

  // Which tiles arrive with an animation, and how long each waits, decided
  // once and then left alone — recomputing it every render would cut the
  // animation off halfway. The wait is staggered by the tile's place among all
  // tiles, capped, so a gallery of eighty clips does not crawl in.
  const greeting = useRef<Map<string, number> | null>(null);
  const known = useRef<Set<string> | null>(null);
  const arrived = useRef(new Set<string>());

  if (greeting.current === null && tiles.length > 0) {
    greeting.current = greeted
      ? new Map()
      : new Map(tiles.slice(0, GREETED_TILES).map((tile, slot) => [tile.key, slot * 30]));
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
    setStartAt(undefined);
    setPlaylist(visible.map((clip) => clip.id));
    touched.current = new Set(visible[index] ? [visible[index].id] : []);
    coming.current = visible[index]?.id ?? null;
    setOpen(index);
  };

  // A clip handed over from the console. The filter has to go first: the clip
  // may not be in whatever was on screen, and the player counts in the list it
  // is given. The gallery then shows that clip's type, in the current order.
  // A viewer already open is asked first, as when it is closed: a running save
  // is waited for, unapplied edits are asked about.
  useEffect(() => {
    if (!focus) return;
    if (!clips.some((c) => c.id === focus.id)) return;
    onFocused?.();
    whenViewerFree(() => handOver(focus));
    // `handOver` reads the library afresh when it runs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focus, clips, onFocused]);

  const handOver = (focus: FocusClip) => {
    const all = useEngine.getState().clips;
    const clip = all.find((c) => c.id === focus.id);
    if (!clip) return;
    const next: ClipFilter = { ...NO_FILTER, kind: kindOf(clip) };
    setFilter(next);
    const list = sortClips(filterClips(all, next), useGallery.getState().sort);
    setPlaylist(list.map((c) => c.id));
    // Wie beim Öffnen aus der Galerie: was angesehen wurde, wird beim Schließen
    // in seinen Spielordner einsortiert. Ohne diese Zeile wäre der Weg über die
    // Konsole der einzige, bei dem die Datei liegen bliebe.
    touched.current = new Set([focus.id]);
    coming.current = focus.id;
    // Vor dem Öffnen, damit der Player die Stelle schon beim ersten Bild hat.
    setStartAt(focus.at);
    setOpen(list.findIndex((c) => c.id === focus.id));
  };

  /** Page on inside the open viewer. Both of them do it the same way. */
  const openIndex = (next: number) => {
    const clip = playing[next];
    if (clip) touched.current.add(clip.id);
    shownId.current = clip?.id ?? null;
    coming.current = null;
    setOpen(next);
  };

  /** On close, catch up on what was not possible during playback: move the
      files of the clips that were viewed into their folder. */
  const closePlayer = () => {
    setOpen(null);
    // Back to the tile the keyboard left — the gallery was out of reach meanwhile.
    requestAnimationFrame(() => {
      const id = restingRef.current;
      if (id) thumbs.current.get(id)?.focus({ preventScroll: true });
    });
    shownId.current = null;
    coming.current = null;
    const seen = [...touched.current];
    touched.current = new Set();
    // Ein inzwischen gelöschter Clip hat keine Datei mehr zum Einsortieren.
    const left = new Set(clips.map((c) => c.id));
    for (const id of seen) if (left.has(id)) void fileClip(id);
  };

  /** Deleting from the player here; it moves on to the neighbour by itself. */
  const deleteFromPlayer = (id: string) => {
    deletedHere.current.add(id);
    void deleteClip(id);
  };

  // `open` ist nur eine Stelle in der Liste. Ändert sich die Liste von außen —
  // die Konsole löscht, die Datenbank wird neu gelesen —, zeigte dieselbe Stelle
  // still einen anderen Clip. Darum hält `shownId` den Clip selbst fest: steht
  // er woanders, wandert `open` mit; ist er woanders gelöscht worden, geht der
  // Player zu. Nur wenn dieser Player selbst gelöscht hat, rückt er zum
  // Nachbarn — und der zählt dann als angesehen und wird beim Schließen
  // einsortiert.
  useEffect(() => {
    if (open === null) return;
    // Gerade geöffnet oder aus der Konsole übergeben: Liste und Stelle kommen
    // erst mit einem der nächsten Rendern an. Bis dahin nichts schließen.
    if (coming.current !== null) {
      const at = playing.findIndex((c) => c.id === coming.current);
      if (at === -1) return;
      shownId.current = coming.current;
      coming.current = null;
      if (at !== open) setOpen(at);
      return;
    }
    // The last clip of the list went, deleted in this player or elsewhere: the
    // viewer is unmounted already and will never call `onClose`, so the gallery
    // closes it itself — or it stays `inert` behind a player that is not there.
    if (playing.length === 0) {
      closePlayer();
      return;
    }
    const was = shownId.current;
    const at = was === null ? -1 : playing.findIndex((c) => c.id === was);
    if (at === -1) {
      if (was !== null && !deletedHere.current.has(was)) {
        closePlayer();
        return;
      }
      const now = playing[Math.min(open, playing.length - 1)];
      shownId.current = now?.id ?? null;
      if (now) touched.current.add(now.id);
    } else if (at !== open) {
      setOpen(at);
    }
  });

  /** Heart on or off — and take the file along right away. */
  const toggleFavorite = async (clip: Clip) => {
    await setFavorite(clip.id, !clip.favorite);
    await fileClip(clip.id);
  };

  // What the tiles call, by clip — one function each for the whole gallery, so
  // a tile whose own props did not change is left alone when the gallery
  // renders (a heart, a re-read library, the selection elsewhere).
  const tileAsk = useCallback(
    (id: string, what: Asking | null) => setAsking(what ? { id, what } : null),
    [],
  );
  const tileRenaming = useCallback((id: string, on: boolean) => setRenaming(on ? id : null), []);
  const tileThumb = useCallback((id: string, node: HTMLButtonElement | null) => {
    if (node) thumbs.current.set(id, node);
    else thumbs.current.delete(id);
  }, []);
  const tileExport = useCallback((clip: Clip) => setExporting(clip), []);
  const tileSelect = useStableHandler((id: string, mods: SelectMods) => selection.select(id, mods));
  const tileOpen = useStableHandler((id: string) => openAt(positions.get(id) ?? 0));
  const tileFavorite = useStableHandler((clip: Clip) => void toggleFavorite(clip));
  const tileDelete = useStableHandler((id: string) => void deleteClip(id));
  const tileDiscard = useStableHandler(
    (clip: Clip) => void (clip.originalAvailable ? discardClipOriginal(clip.id) : bakeClipCam(clip.id)),
  );
  const tileMenu = useStableHandler((event: React.MouseEvent, clip: Clip) => {
    // If the cursor is in the name field the menu belongs to the text —
    // `TextMenu` takes care of that on its own.
    if (isTextField(event.target)) return;
    clipMenu(event, clip, {
      onOpen: () => openAt(positions.get(clip.id) ?? 0),
      onRename: () => setRenaming(clip.id),
      onDelete: () => setAsking({ id: clip.id, what: "delete" }),
      onDiscardOriginal: () => setAsking({ id: clip.id, what: "discard" }),
      onExport: () => setExporting(clip),
    });
  });
  /** One clock for every tile's "5m ago" in this render. */
  const now = Date.now();

  return (
    <div className="space-y-6">
      {/* While the viewer is up the gallery behind it is out of the tab order. */}
      <header className="pt-10" inert={open !== null}>
        <h1 className="display text-4xl">Clips</h1>
      </header>

      <div className="space-y-3" inert={open !== null}>
        <div className="flex items-center gap-3">
          {/* Not `setFilter`: the search also narrows Recently deleted, and
              typing must not close it. */}
          <SearchField
            value={filter.query}
            onChange={(query) => useGallery.setState({ filter: { ...filter, query } })}
          />
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
          {!inTrash && (
            <SortButton sort={sort} onChange={(next) => useGallery.setState({ sort: next })} />
          )}
        </div>

        <FilterBar
          clips={clips}
          favorites={favorites}
          kinds={kinds}
          deleted={trash.length}
          filter={filter}
          inTrash={inTrash}
          onChange={setFilter}
          onTrash={() => useGallery.setState({ inTrash: !inTrash })}
          onRemove={clearGame}
        />
      </div>

      {inTrash ? (
        <TrashList trash={trash} query={filter.query} />
      ) : tiles.length === 0 ? (
        <EmptyCard total={clips.length} onClear={() => setFilter(NO_FILTER)} />
      ) : (
        // One wrapper around every day, so a tile that changes day on a new
        // sort still travels instead of jumping.
        <div
          ref={grid}
          className="space-y-7 pb-10"
          inert={open !== null}
          onKeyDown={onGridKeyDown}
          // Space on a button acts when it is let go.
          onKeyUp={(event) => {
            if (event.key === " " && (event.target as HTMLElement).matches("[data-tile-open]"))
              event.preventDefault();
          }}
          onFocus={(event) => {
            const id = (event.target as HTMLElement).closest<HTMLElement>("[data-flip]")?.dataset.flip;
            if (id) setResting(id);
          }}
        >
          {shownGroups.map((group, at) => (
            <section key={group.key}>
              {group.label && (
                <DayHeading
                  label={group.label}
                  count={groups[at].items.filter((tile) => !tile.leaving).length}
                />
              )}
              <div className="grid grid-cols-3 gap-4">
                {group.items.map(({ item: clip, key, leaving }) => {
                  const wait = greeting.current?.get(key) ?? null;
                  return (
                    <ClipCard
                      key={key}
                      clip={clip}
                      flipKey={key}
                      leaving={leaving}
                      arriving={wait !== null || arrived.current.has(key)}
                      delay={wait}
                      ago={formatAgo(clip.createdAt, now)}
                      link={links[clip.id]}
                      asking={asking?.id === clip.id ? asking.what : null}
                      onAsk={tileAsk}
                      renaming={renaming === clip.id}
                      onRenaming={tileRenaming}
                      thumbRef={tileThumb}
                      selected={selection.selected.has(clip.id)}
                      selecting={selecting}
                      onSelect={tileSelect}
                      onOpen={tileOpen}
                      onExport={tileExport}
                      onFavorite={tileFavorite}
                      onDelete={tileDelete}
                      onDiscard={tileDiscard}
                      onContextMenu={tileMenu}
                    />
                  );
                })}
              </div>
            </section>
          ))}
        </div>
      )}

      {/* Which of the two opens is decided per entry, not per playlist: paging
          through a gallery of both leads from a clip into a picture and back. */}
      {open !== null && playing.length > 0 && (
        <ErrorBoundary
          resetKey={playing[Math.min(open, playing.length - 1)]?.id}
          overlay
          onBack={closePlayer}
        >
          {playing[Math.min(open, playing.length - 1)]?.screenshot ? (
            <ShotViewer
              clips={playing}
              index={Math.min(open, playing.length - 1)}
              onIndexChange={openIndex}
              onClose={closePlayer}
              onDelete={deleteFromPlayer}
              originOf={originOf}
            />
          ) : (
            <ClipPlayer
              clips={playing}
              index={Math.min(open, playing.length - 1)}
              onIndexChange={openIndex}
              onClose={closePlayer}
              onDelete={deleteFromPlayer}
              onOpenMixer={() => onNavigate("audio")}
              originOf={originOf}
              startAt={startAt}
            />
          )}
        </ErrorBoundary>
      )}

      {selecting && open === null && !inTrash && (
        <SelectionBar
          clips={clips.filter((clip) => selection.selected.has(clip.id))}
          links={links}
          onClear={selection.clear}
          onConfirming={(on) => (confirmingSelection.current = on)}
          onFavorite={async (clips, favorite) => {
            for (const clip of clips) {
              if (clip.favorite !== favorite) await toggleFavorite(clip);
            }
          }}
          onDelete={async (clips) => {
            selection.clear();
            bulk.current = clips.length > BULK_ANIMATED;
            try {
              await deleteClips(clips.map((clip) => clip.id));
            } finally {
              bulk.current = false;
            }
            // The store swallows a refused delete and puts the clip back, so
            // count what is really gone.
            const left = new Set(useEngine.getState().clips.map((c) => c.id));
            const moved = clips.filter((clip) => !left.has(clip.id)).length;
            const what = moved === 1 ? "1 clip" : `${moved} clips`;
            if (moved === clips.length) toast("ok", `${what} moved to Recently deleted`);
            else if (moved === 0) toast("error", "Could not delete the selected clips");
            else toast("error", `Only ${moved} of ${clips.length} clips moved to Recently deleted`);
          }}
        />
      )}

      {exporting && (
        <ExportDialog clip={exporting} onClose={() => setExporting(null)} />
      )}
    </div>
  );
}

/** "Today · 3 clips" over a day's tiles. */
const DayHeading = memo(function DayHeading({ label, count }: { label: string; count: number }) {
  return (
    <h2 className="mb-3 flex items-baseline gap-2.5 text-[15px] font-semibold text-ink/80">
      {label}
      <span className="text-[13px] font-normal text-ink-faint tabular-nums">
        {count === 1 ? "1 clip" : `${count} clips`}
      </span>
    </h2>
  );
});

/** What a tile is asking about, in its own place over the thumbnail. */
type Asking = "delete" | "discard";

interface ClipCardProps {
  clip: Clip;
  /** The tile's key for `useFlip` — the presence key, the clip id. */
  flipKey: string;
  leaving: boolean;
  /** Plays the arrival — the first greeting or a clip saved just now. */
  arriving: boolean;
  /** How long the arrival waits, for the staggered greeting. */
  delay: number | null;
  /** "5m ago" — worked out by the gallery, so the tile changes when the text does. */
  ago: string;
  link: Link | undefined;
  asking: Asking | null;
  onAsk: (id: string, what: Asking | null) => void;
  renaming: boolean;
  onRenaming: (id: string, on: boolean) => void;
  thumbRef: (id: string, node: HTMLButtonElement | null) => void;
  /** In the selection — a ring and a filled check. */
  selected: boolean;
  /** Something is selected somewhere: a click picks instead of opening. */
  selecting: boolean;
  /** Ctrl-click and Shift-click anywhere on the tile, any click on the picture
   *  while a selection exists. */
  onSelect: (id: string, mods: SelectMods) => void;
  onOpen: (id: string) => void;
  onExport: (clip: Clip) => void;
  onFavorite: (clip: Clip) => void;
  onDelete: (id: string) => void;
  onDiscard: (clip: Clip) => void;
  onContextMenu: (event: React.MouseEvent, clip: Clip) => void;
}

/**
 * A tile renders again only when its own props change. The clip is compared
 * by content: a re-read library hands over a new object for every clip, and
 * the unchanged ones should stay as they are. The handlers are the gallery's
 * shared ones and take the clip, so they never differ between renders.
 */
function sameCardProps(a: ClipCardProps, b: ClipCardProps): boolean {
  const keys = Object.keys(a) as Array<keyof ClipCardProps>;
  if (keys.length !== Object.keys(b).length) return false;
  return keys.every((key) => (key === "clip" ? sameData(a.clip, b.clip) : Object.is(a[key], b[key])));
}

/**
 * One tile of the gallery: thumbnail with its pills, the heart, folder and bin
 * on hover, the name to rename in place, and Open and Share.
 *
 * Everything it changes goes through its props, so the gallery stays the one
 * place that knows which tile is asking, renaming or open.
 */
const ClipCard = memo(function ClipCard({
  clip,
  flipKey,
  leaving,
  arriving,
  delay,
  ago,
  link,
  asking,
  onAsk,
  renaming,
  onRenaming,
  thumbRef,
  selected,
  selecting,
  onSelect,
  onOpen,
  onExport,
  onFavorite,
  onDelete,
  onDiscard,
  onContextMenu,
}: ClipCardProps) {
  const id = clip.id;
  const thumb = useCallback((node: HTMLButtonElement | null) => thumbRef(id, node), [thumbRef, id]);
  const editing = useCallback((on: boolean) => onRenaming(id, on), [onRenaming, id]);
  // A focused control that goes away with a render — a question answered with
  // Escape — takes the focus along without a blur, so the mark is checked here.
  const box = useRef<HTMLElement | null>(null);
  useLayoutEffect(() => {
    const node = box.current;
    if (node?.hasAttribute("data-kbd-focus") && !node.contains(document.activeElement)) {
      node.removeAttribute("data-kbd-focus");
    }
  });
  return (
    <Card
      data-flip={flipKey}
      data-tile
      interactive={!leaving}
      className={cn(
        "group overflow-hidden",
        leaving ? "cb-tile-out" : arriving && "cb-tile-in",
        selected && "border-accent shadow-[0_0_0_1px_var(--color-accent)] hover:border-accent",
      )}
      style={delay !== null ? { animationDelay: `${delay}ms` } : undefined}
      onContextMenu={(event) => onContextMenu(event, clip)}
      // Keyboard focus anywhere in the tile shows its tools. An attribute, not
      // `:has(:focus-visible)`: a rule like that made every node added anywhere
      // on the page restyle the whole document.
      onFocus={(event) => {
        box.current = event.currentTarget;
        event.currentTarget.toggleAttribute("data-kbd-focus", event.target.matches(":focus-visible"));
      }}
      onBlur={(event) => event.currentTarget.removeAttribute("data-kbd-focus")}
      // Before the buttons inside see it, so Ctrl-click on Open or Share picks
      // the tile too instead of acting.
      onClickCapture={(event) => {
        // With a selection, any plain click on the tile picks it too — Open,
        // Share and the heart included — like a file manager in selection mode.
        // Enter on a focused button arrives as a click without a pointer
        // (detail 0): that one acts, Space is what picks from the keyboard.
        const keyboard = event.detail === 0 && !(event.ctrlKey || event.metaKey || event.shiftKey);
        if (leaving || keyboard || !(selecting || event.ctrlKey || event.metaKey || event.shiftKey)) return;
        if (isTextField(event.target)) return;
        // The delete question's own buttons act, even in selection mode.
        if (asking && (event.target as HTMLElement).closest("[data-tile-tools]")) return;
        event.preventDefault();
        event.stopPropagation();
        onSelect(id, { shift: event.shiftKey, toggle: true });
      }}
      // Shift-click would start a text selection across the tiles.
      onMouseDown={(event) => {
        if (event.shiftKey && !isTextField(event.target)) event.preventDefault();
      }}
    >
      <div className="relative">
        <button
          ref={thumb}
          data-tile-open
          data-ring-inset
          onClick={() => onOpen(id)}
          aria-label={`${clip.screenshot ? "Open" : "Play"} ${
            clip.game ??
            (clip.screenshot ? "screenshot" : clip.recording ? "recording" : "clip")
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
              // Die Galerie hält alle Clips auf einmal, und jedes
              // dekodierte Bild ist ein halbes Megabyte im Speicher —
              // bei über hundert Kacheln war das der größte Posten in
              // einem WebView, das Chromium wegen der Overlay-Flags
              // ohnehin nie aufräumt. `lazy` dekodiert erst, was in die
              // Nähe des Sichtfensters kommt.
              loading="lazy"
              decoding="async"
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
              {clip.facecam?.state === "pending" && (
                <Pill title="The cam is being added to this clip">Adding cam</Pill>
              )}
              {clip.original && (
                <Pill title="Trimmed — the original sits beside it">
                  <IconScissors className="h-3 w-3" />
                </Pill>
              )}
              {link && (
                <Pill title={`Shared as a link · ${timeLeft(link)}`}>
                  <IconLink className="h-3 w-3" />
                </Pill>
              )}
              {/* A still has no length. The camera says what the
                  duration would have said on a clip. */}
              {clip.screenshot ? (
                <Pill title="Screenshot">
                  <IconCamera className="h-3 w-3" />
                </Pill>
              ) : clip.recording ? (
                // The red mark tells a recording from a clip at a
                // glance, before the length does.
                <Pill title="Recording">
                  <IconRecord className="h-3 w-3 text-live" />
                  {formatDuration(clip.durationMs)}
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
          aria-label={clip.favorite ? "Remove from favorites" : "Add to favorites"}
          aria-pressed={clip.favorite}
          title={
            clip.favorite
              ? 'Favorite — the file lives in the "Favorites" folder'
              : "Add to favorites"
          }
          onClick={() => onFavorite(clip)}
          className={cn(
            "absolute top-3 left-3 grid h-8 w-8 place-items-center rounded-pill",
            "bg-black/50 backdrop-blur-md transition",
            clip.favorite
              ? "text-live"
              : "text-white/70 opacity-0 group-hover:opacity-100 group-data-[kbd-focus]:opacity-100 hover:text-white",
          )}
        >
          <HeartBurst favorite={clip.favorite} />
        </button>
        {selecting && !leaving && (
          // Ring while nothing is picked, filled once it is. Not a button: the
          // click goes to the picture and the tile, which already toggle.
          <span
            aria-hidden
            className={cn(
              "pointer-events-none absolute top-3 right-3 grid h-[26px] w-[26px] place-items-center",
              "rounded-pill border-2 text-white",
              selected ? "border-accent bg-accent" : "border-white/75 bg-black/35",
            )}
          >
            {selected && <IconCheck className="h-3.5 w-3.5" />}
          </span>
        )}
        <div
          data-tile-tools
          className={cn(
            "absolute top-3 right-3 flex gap-1.5 transition-opacity",
            selecting && (asking ? "right-12" : "hidden"),
            // While the question stands it has to stay up, even once
            // the cursor has wandered off the tile.
            asking
              ? "opacity-100"
              : "opacity-0 group-hover:opacity-100 group-data-[kbd-focus]:opacity-100",
          )}
        >
          {asking === "delete" ? (
            <ConfirmDelete
              origin="right"
              // Its share link goes along — worth saying before, not after.
              question={link ? "Delete with its link?" : undefined}
              onConfirm={() => {
                onAsk(id, null);
                onDelete(id);
              }}
              onCancel={() => onAsk(id, null)}
            />
          ) : asking === "discard" ? (
            <ConfirmDelete
              origin="right"
              question={
                clip.originalAvailable ? "Throw the recording away?" : "Bake the cam into the clip?"
              }
              confirmLabel={clip.originalAvailable ? "Throw away" : "Bake"}
              confirmTitle={freeSpaceTitle(clip)}
              onConfirm={() => {
                onAsk(id, null);
                onDiscard(clip);
              }}
              onCancel={() => onAsk(id, null)}
            />
          ) : (
            <>
              <IconAction label="Show in folder" onClick={() => inTauri && api.revealClip(clip.id)}>
                <IconFolder className="h-4 w-4" />
              </IconAction>
              <IconAction label="Delete clip" danger onClick={() => onAsk(id, "delete")}>
                <IconTrash className="h-4 w-4" />
              </IconAction>
            </>
          )}
        </div>
      </div>
      <div className="p-4">
        <NameField clip={clip} editing={renaming} onEditing={editing} />
        <p className="mt-1 truncate text-xs text-ink-muted">
          {clip.game ? `${clip.game} · ` : ""}
          {ago} ·{" "}
          {/* A still is measured by its edges, not by "1080p" — that is
              a word about video. */}
          {clip.screenshot ? `${clip.width} × ${clip.height}` : `${clip.height}p`} ·{" "}
          {formatSize(clip.sizeBytes)}
        </p>
        {clip.description && (
          <p className="mt-1 truncate text-xs text-ink-faint">{clip.description}</p>
        )}
        {/* Viewing and editing are one screen; everything that sends the
            clip somewhere is one menu beside it. */}
        <div className="mt-3 flex gap-2">
          <Button size="sm" onClick={() => onOpen(id)}>
            Open
          </Button>
          <ShareMenuButton clip={clip} variant="secondary" onExport={() => onExport(clip)} />
        </div>
      </div>
    </Card>
  );
}, sameCardProps);

/**
 * What the selected clips can do together: heart them, send them to a friend,
 * delete them. The navigation's glass pill, floating at the bottom — centred,
 * so it stays clear of the toasts and transfers in the right-hand corner.
 */
function SelectionBar({
  clips,
  links,
  onClear,
  onConfirming,
  onFavorite,
  onDelete,
}: {
  clips: Clip[];
  links: Record<string, Link>;
  onClear: () => void;
  /** Tells the gallery a question is up, so Escape answers it and nothing else. */
  onConfirming: (on: boolean) => void;
  onFavorite: (clips: Clip[], favorite: boolean) => Promise<void>;
  onDelete: (clips: Clip[]) => Promise<void>;
}) {
  const signedIn = useFriends((s) => s.signedIn);
  const pickFriend = useShare((s) => s.pick);
  const [confirming, setConfirming] = useState(false);
  useEffect(() => {
    onConfirming(confirming);
    return () => onConfirming(false);
  }, [confirming, onConfirming]);

  // The transfer cards (z 55) stand bottom right; on a narrow window the
  // centred bar would run under them, so it lifts clear — as the toasts do.
  const [lift, setLift] = useState<number | null>(null);
  useLayoutEffect(() => {
    const measure = () => {
      const cards = document.querySelector("[data-transfers]");
      const next = cards ? Math.round(window.innerHeight - cards.getBoundingClientRect().top + 8) : null;
      setLift((now) => (now === next ? now : next));
    };
    measure();
    const timer = window.setInterval(measure, 300);
    return () => window.clearInterval(timer);
  }, []);

  const count = clips.length;
  const heart = favoriteTarget(clips);
  const withLinks = clips.some((clip) => links[clip.id]);

  // In the body: the page sits in a layer that scrolls and moves, and a bar
  // fixed inside it would scroll away with the clips.
  return createPortal(
    <div
      role="toolbar"
      aria-label="Selected clips"
      className="fixed bottom-7 left-1/2 z-[45] flex -translate-x-1/2 items-center gap-1.5 rounded-pill
        border border-white/10 py-1.5 pr-2 pl-[18px] shadow-[0_8px_32px_rgba(0,0,0,0.4)]"
      style={{
        background: "var(--glass)",
        backdropFilter: "blur(20px)",
        ...(lift !== null && window.innerWidth < 1400 ? { bottom: Math.max(28, lift) } : null),
      }}
    >
      <b className="mr-2 text-[13px] font-semibold tabular-nums">{count} selected</b>
      <Button
        variant="ghost"
        size="sm"
        className="text-white"
        onClick={() => void onFavorite(clips, heart)}
        title={heart ? "Add to favorites" : "Remove from favorites"}
      >
        <IconHeart filled={!heart} className="mr-1.5 h-3.5 w-3.5" />
        Favorite
      </Button>
      {signedIn && (
        <Button
          variant="ghost"
          size="sm"
          className="text-white"
          disabled={count !== 1}
          title={count === 1 ? undefined : "Send one clip at a time"}
          onClick={() => pickFriend(clips[0])}
        >
          <IconSend className="mr-1.5 h-3.5 w-3.5" />
          Send to a friend
        </Button>
      )}
      {confirming ? (
        <ConfirmDelete
          origin="left"
          question={withLinks ? `Delete ${count} with links?` : `Delete ${count}?`}
          confirmTitle="Delete — they stay in Recently deleted for 30 days"
          cancelLabel="Keep the clips"
          onConfirm={() => {
            setConfirming(false);
            void onDelete(clips);
          }}
          onCancel={() => setConfirming(false)}
        />
      ) : (
        <Button variant="ghost" size="sm" className="text-live" onClick={() => setConfirming(true)}>
          Delete
        </Button>
      )}
      <span aria-hidden className="mx-1 h-5 w-px bg-white/10" />
      <Button variant="ghost" size="sm" aria-label="Clear selection" title="Clear selection (Escape)" onClick={onClear}>
        <IconClose className="h-3.5 w-3.5" />
      </Button>
    </div>,
    document.body,
  );
}

/** Newest ▾ — the order of the gallery, in the context menu's look. */
function SortButton({ sort, onChange }: { sort: ClipSort; onChange: (sort: ClipSort) => void }) {
  const menu = useMenu();
  return (
    <Button
      variant="ghost"
      size="sm"
      aria-haspopup="menu"
      className="shrink-0"
      onClick={(event) => {
        const box = event.currentTarget.getBoundingClientRect();
        menu.open(
          {
            clientX: box.right,
            clientY: box.bottom + 6,
            preventDefault: () => {},
            stopPropagation: () => {},
          },
          SORTS.map(({ key, label }) => ({
            kind: "item",
            label,
            icon: key === sort ? <IconCheck className="h-3.5 w-3.5" /> : undefined,
            onSelect: () => onChange(key),
          })),
          { alignRight: true, anchor: box, gap: 6 },
        );
      }}
    >
      {SORTS.find((s) => s.key === sort)?.label} ▾
    </Button>
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

const KINDS: Array<{ key: ClipKind; label: string }> = [
  { key: "all", label: "All" },
  { key: "clips", label: "Clips" },
  { key: "recordings", label: "Recordings" },
  { key: "screenshots", label: "Screenshots" },
];

/**
 * The type segment and the filters that combine with it, always on one line:
 * Favorites, "Game ▾" and "Tags ▾" with their lists, and Recently deleted at
 * the end. A chosen filter turns its button white with a × that clears it.
 */
function FilterBar({
  clips,
  favorites,
  kinds,
  deleted,
  filter,
  inTrash,
  onChange,
  onTrash,
  onRemove,
}: {
  clips: Clip[];
  favorites: number;
  kinds: Record<ClipKind, number>;
  deleted: number;
  filter: ClipFilter;
  /** Recently deleted is open: nothing in the segment or the buttons is lit. */
  inTrash: boolean;
  onChange: (filter: ClipFilter) => void;
  onTrash: () => void;
  /** Detaches a game name from its clips. */
  onRemove: (game: string) => void;
}) {
  const row = useRef<HTMLDivElement>(null);
  const gameButton = useRef<HTMLDivElement>(null);
  const tagButton = useRef<HTMLDivElement>(null);
  const [opened, setOpened] = useState<"game" | "tag" | null>(null);

  // Counted within the chosen type, like the segment's own numbers narrow it.
  const gameList = useMemo(
    () => gameFacet(clips, filter.kind, filter.games),
    [clips, filter.kind, filter.games],
  );
  const tagList = useMemo(() => tagFacet(clips, filter.kind, filter.tags), [clips, filter.kind, filter.tags]);

  // Recently deleted shows a list of its own; the filters wait behind it,
  // unlit, and one picked from there brings the gallery back with it on.
  const games = inTrash ? [] : filter.games;
  const noGame = !inTrash && filter.noGame;
  const tags = inTrash ? [] : filter.tags;
  const chosenGames = [...games, ...(noGame ? ["No game"] : [])];
  const pick = (list: string[], name: string) =>
    inTrash ? (list.includes(name) ? list : [...list, name]) : toggled(list, name);

  const gameLabel = facetLabel("game", chosenGames);
  const tagLabel = facetLabel("tag", tags);
  // A longer label pushes the buttons after it along — a slide, not a jump.
  useFlip(row, [favorites > 0, gameLabel, chosenGames.length > 0, tagLabel, tags.length > 0].join("|"));

  return (
    <div ref={row} className="flex items-center gap-2 py-0.5">
      <div className="shrink-0 pr-1.5">
        <Segmented
          value={inTrash ? null : filter.kind}
          onChange={(kind) => onChange({ ...filter, kind })}
          options={KINDS.map(({ key, label }) => ({
            key,
            label: (
              <>
                {label}
                <span
                  className={cn(
                    "ml-1.5 text-xs",
                    !inTrash && filter.kind === key ? "text-black/45" : "text-ink-faint",
                  )}
                >
                  {kinds[key]}
                </span>
              </>
            ),
          }))}
        />
      </div>
      {favorites > 0 && (
        <Chip
          id="fav"
          active={!inTrash && filter.favorites}
          count={favorites}
          icon={
            <IconHeart
              filled
              className={cn("h-3.5 w-3.5", !inTrash && filter.favorites ? "text-black/70" : "text-live")}
            />
          }
          onClick={() => onChange({ ...filter, favorites: inTrash || !filter.favorites })}
        >
          Favorites
        </Chip>
      )}
      <FacetButton
        id="game"
        anchor={gameButton}
        label={gameLabel}
        noun="game"
        active={chosenGames.length > 0}
        open={opened === "game"}
        onOpen={() => setOpened((now) => (now === "game" ? null : "game"))}
        onClear={() => onChange({ ...filter, games: [], noGame: false })}
      />
      <FacetButton
        id="tag"
        anchor={tagButton}
        label={tagLabel}
        noun="tag"
        active={tags.length > 0}
        open={opened === "tag"}
        onOpen={() => setOpened((now) => (now === "tag" ? null : "tag"))}
        onClear={() => onChange({ ...filter, tags: [] })}
      />
      {deleted > 0 && (
        <Button
          variant={inTrash ? "primary" : "ghost"}
          size="sm"
          aria-pressed={inTrash}
          className="ml-auto shrink-0"
          icon={<IconTrash className="h-3.5 w-3.5" />}
          onClick={onTrash}
        >
          Recently deleted
          <span className={cn("tabular-nums", inTrash ? "text-black/45" : "text-ink-faint")}>
            {deleted}
          </span>
        </Button>
      )}

      {opened === "game" && (
        <FacetPopover
          anchor={gameButton}
          noun="games"
          values={gameList.values.map((v) => ({ ...v, checked: games.includes(v.name) }))}
          noGame={gameList.noGame > 0 || noGame ? { count: gameList.noGame, checked: noGame } : null}
          onToggle={(name) => onChange({ ...filter, games: pick(filter.games, name) })}
          onToggleNoGame={() => onChange({ ...filter, noGame: inTrash || !filter.noGame })}
          onClear={() => onChange({ ...filter, games: [], noGame: false })}
          onRemove={onRemove}
          onClose={() => setOpened(null)}
        />
      )}
      {opened === "tag" && (
        <FacetPopover
          anchor={tagButton}
          noun="tags"
          values={tagList.map((v) => ({ ...v, checked: tags.includes(v.name) }))}
          onToggle={(name) => onChange({ ...filter, tags: pick(filter.tags, name) })}
          onClear={() => onChange({ ...filter, tags: [] })}
          onClose={() => setOpened(null)}
        />
      )}
    </div>
  );
}

/** A single small nod when a filter takes over — not on mount, or the whole
 *  bar would twitch every time the gallery opens. */
function useNod(box: React.RefObject<HTMLElement | null>, active: boolean) {
  const was = useRef(active);
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
  }, [box, active]);
}

/**
 * "Game ▾" or "Tags ▾" in the chip look. Once something is chosen it is white
 * like a chosen chip, names it ("Valorant", "2 games") and has a × that clears
 * it without opening the list.
 */
function FacetButton({
  id,
  anchor,
  label,
  noun,
  active,
  open,
  onOpen,
  onClear,
}: {
  id: string;
  /** The whole pill: the list hangs from it. */
  anchor: React.RefObject<HTMLDivElement | null>;
  label: string;
  noun: string;
  active: boolean;
  open: boolean;
  onOpen: () => void;
  onClear: () => void;
}) {
  useNod(anchor, active);
  return (
    <div
      ref={anchor}
      data-flip={id}
      className={cn(
        "flex h-8 items-center rounded-pill border transition-colors duration-150",
        // The one thing in the row that gives way, so it stays one line with
        // long names chosen at the narrowest window. A short name keeps itself:
        // "c…" would say nothing.
        label.length > 8 ? "min-w-0" : "shrink-0",
        active
          ? "border-white bg-white text-black"
          : cn(
              "border-line bg-surface hover:border-line-strong hover:text-ink",
              open ? "border-line-strong text-ink" : "text-ink-muted",
            ),
      )}
    >
      <button
        aria-haspopup="dialog"
        aria-expanded={open}
        title={active ? label : undefined}
        onClick={onOpen}
        className={cn(
          "flex h-full min-w-0 items-center gap-1.5 rounded-pill pl-4 text-[13px] font-medium",
          active ? "pr-1" : "pr-4",
        )}
      >
        <span className="max-w-[180px] truncate">{label}</span>
        <span aria-hidden className="shrink-0">
          ▾
        </span>
      </button>
      {active && (
        <button
          aria-label={`Clear the ${noun} filter`}
          title={`Clear the ${noun} filter`}
          onClick={onClear}
          className="mr-1 grid h-6 w-6 shrink-0 place-items-center rounded-pill text-black/70
            transition-colors hover:bg-black/10 hover:text-black"
        >
          <IconClose className="h-3 w-3" />
        </button>
      )}
    </div>
  );
}

/** Whole days until `expiresAt`, at least one while it is still there. */
function daysLeft(expiresAt: number): number {
  return Math.max(1, Math.ceil((expiresAt - Date.now()) / (24 * 60 * 60 * 1000)));
}

/** Empty gallery or nothing matching: the library is empty, or search and filters cut it to nothing. */
function EmptyCard({ total, onClear }: { total: number; onClear: () => void }) {
  const config = useEngine((s) => s.config);
  const saveKey = shownHotkey(config, "saveClip");
  if (total > 0) {
    return (
      <Card className="flex h-56 flex-col items-center justify-center gap-3 text-sm text-ink-muted">
        Nothing matches
        <Button size="sm" variant="ghost" onClick={onClear}>
          Clear
        </Button>
      </Card>
    );
  }
  return (
    <Card className="flex h-56 items-center justify-center text-sm text-ink-muted">
      {saveKey.value ? (
        <>
          No clips yet — start the buffer and press{" "}
          <kbd className="mx-1 rounded-inner border border-line bg-elevated px-2 py-0.5 text-xs">
            {hotkeyLabel(saveKey.value, saveKey.family)}
          </kbd>
        </>
      ) : (
        "No clips yet — start the buffer, then save one"
      )}
    </Card>
  );
}

/**
 * Recently deleted: each clip with the days it has left, Restore and Delete
 * forever, and Empty all above them.
 */
function TrashList({ trash, query }: { trash: Trashed[]; query: string }) {
  const { restoreClip, purgeClip, emptyTrash } = useEngine(
    useShallow((s) => ({
      restoreClip: s.restoreClip,
      purgeClip: s.purgeClip,
      emptyTrash: s.emptyTrash,
    })),
  );
  const [confirming, setConfirming] = useState<string | null>(null);
  const [emptying, setEmptying] = useState(false);
  const needle = query.toLowerCase();
  const shown = trash.filter(({ clip }) =>
    !needle ||
    [fileName(clip.path), clip.title ?? "", clip.game ?? "", ...clip.tags]
      .join(" ")
      .toLowerCase()
      .includes(needle),
  );
  const bytes = trash.reduce((sum, t) => sum + t.clip.sizeBytes, 0);

  return (
    <div className="space-y-4 pb-10">
      <div className="flex h-8 items-center gap-3">
        <span className="text-sm text-ink-muted">
          Kept for 30 days · {formatSize(bytes)}
        </span>
        <span className="ml-auto">
          {emptying ? (
            <ConfirmDelete
              question={`Delete all ${trash.length} for good?`}
              origin="right"
              confirmLabel="Empty Recently deleted"
              confirmTitle="Delete all of them for good"
              cancelLabel="Keep them"
              onConfirm={() => {
                setEmptying(false);
                void emptyTrash();
              }}
              onCancel={() => setEmptying(false)}
            />
          ) : (
            <Button variant="danger" size="sm" icon={<IconTrash className="h-3.5 w-3.5" />} onClick={() => setEmptying(true)}>
              Empty all
            </Button>
          )}
        </span>
      </div>

      {shown.length === 0 ? (
        <Card className="grid h-56 place-items-center text-sm text-ink-muted">
          {trash.length === 0 ? "Nothing deleted" : "Nothing matches"}
        </Card>
      ) : (
        <div className="grid grid-cols-3 gap-4">
          {shown.map(({ clip, expiresAt }) => {
            const days = daysLeft(expiresAt);
            return (
              <Card key={clip.id} className="overflow-hidden">
                <div className="relative aspect-video w-full bg-gradient-to-br from-accent-deep/40 to-black">
                  {clip.thumbPath && (
                    <img
                      src={`${fileUrl(clip.thumbPath)}?v=${clip.sizeBytes}`}
                      alt=""
                      loading="lazy"
                      decoding="async"
                      className="h-full w-full object-cover opacity-60 grayscale"
                    />
                  )}
                  <span className="absolute inset-x-3 bottom-3 flex items-end justify-between gap-2">
                    <Pill className="min-w-0">
                      <span className="min-w-0 truncate">{clip.game ?? "Unknown"}</span>
                    </Pill>
                    <Pill className={days <= 3 ? "text-live" : undefined}>
                      {days === 1 ? "1 day left" : `${days} days left`}
                    </Pill>
                  </span>
                </div>
                <div className="flex items-center gap-2 p-3">
                  <span className="min-w-0 flex-1 truncate text-sm" title={clipName(clip)}>
                    {clipName(clip)}
                  </span>
                  {confirming === clip.id ? (
                    <ConfirmDelete
                      question="For good?"
                      origin="right"
                      confirmTitle="Delete forever — it cannot be restored"
                      onConfirm={() => {
                        setConfirming(null);
                        void purgeClip(clip.id);
                      }}
                      onCancel={() => setConfirming(null)}
                    />
                  ) : (
                    <>
                      <Button size="sm" onClick={() => void restoreClip(clip.id)}>
                        Restore
                      </Button>
                      <button
                        aria-label="Delete forever"
                        title="Delete forever"
                        onClick={() => setConfirming(clip.id)}
                        className="grid h-8 w-8 shrink-0 place-items-center rounded-pill text-ink-muted hover:bg-live/15 hover:text-live"
                      >
                        <IconTrash className="h-4 w-4" />
                      </button>
                    </>
                  )}
                </div>
              </Card>
            );
          })}
        </div>
      )}
    </div>
  );
}

/** The Favorites pill, on or off. A chosen one is white with a × — the whole
 *  pill is the button that switches it off again. */
function Chip({
  id,
  active,
  count,
  icon,
  onClick,
  children,
}: {
  id: string;
  active: boolean;
  count: number;
  icon?: React.ReactNode;
  onClick: () => void;
  children: string;
}) {
  const box = useRef<HTMLDivElement>(null);
  useNod(box, active);

  return (
    <div
      ref={box}
      data-flip={id}
      className={cn(
        "flex h-8 shrink-0 items-center rounded-pill border",
        "transition-colors duration-150",
        active
          ? "border-white bg-white text-black"
          : "border-line bg-surface text-ink-muted hover:border-line-strong hover:text-ink",
      )}
    >
      <button
        onClick={onClick}
        aria-pressed={active}
        title={active ? `${children} — click to turn off` : children}
        className={cn(
          "flex h-full min-w-0 items-center gap-1.5 rounded-pill pl-4 text-[13px] font-medium",
          active ? "pr-1" : "pr-4",
        )}
      >
        {icon}
        <span className="max-w-[180px] truncate">{children}</span>
        <span className={cn("tabular-nums", active ? "text-black/45" : "text-ink-faint")}>
          {count}
        </span>
        {active && (
          <span aria-hidden className="grid h-6 w-6 shrink-0 place-items-center text-black/70">
            <IconClose className="h-3 w-3" />
          </span>
        )}
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

/** What "Free up space" costs on this clip, said before it happens. */
function freeSpaceTitle(clip: Clip): string {
  const cam = clip.facecam?.state === "ready";
  if (!clip.originalAvailable) return "The cam can no longer be moved in this clip.";
  return cam
    ? "The trim stays, undo goes. The cam can no longer be moved in this clip."
    : "The trim stays, undo goes";
}
