import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { create } from "zustand";
import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card, Pill } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Segmented } from "@/components/ui/Controls";
import { useMenu, type MenuEntry } from "@/components/ui/Menu";
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
import { usePresence } from "@/lib/usePresence";
import { useCountUp } from "@/lib/useCountUp";
import { useClipSelection, type SelectMods } from "@/lib/useClipSelection";
import { moveInGrid, NAV_KEYS, type NavKey } from "@/lib/gridNav";
import { favoriteTarget } from "@/lib/clipSelect";
import { toast } from "@/lib/toastBus";
import { useFriends } from "@/lib/friends";
import { useShare } from "@/lib/share";
import {
  NO_FILTER,
  filterClips,
  fitChips,
  groupByDay,
  isFiltered,
  kindCounts,
  kindOf,
  pruneFilter,
  sortClips,
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
/** The gallery's columns — the arrow keys count rows in them. */
const GRID_COLUMNS = 3;

/**
 * Makes the grid one tab stop: every control of every tile but the resting one
 * leaves the tab order. Only what this sets is taken back, so a control with a
 * `tabIndex` of its own is left alone.
 */
function applyRoving(grid: HTMLElement, resting: string | null) {
  const controls = "button, a[href], input, select, textarea, [tabindex], [data-roving]";
  for (const tile of grid.querySelectorAll<HTMLElement>("[data-flip]")) {
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
}

/**
 * Whether the gallery has already introduced itself in this session.
 *
 * The staggered arrival is a greeting, not a habit: coming back from another
 * tab for the fourth time should simply show the clips.
 */
let greeted = false;

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
  const { clips, trash, deleteClip, discardClipOriginal, clearGame, setFavorite, fileClip } =
    useEngine(
      useShallow((s) => ({
        clips: s.clips,
        trash: s.trash,
        deleteClip: s.deleteClip,
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

  /** Tags by how often they are used, like the games. */
  const tags = useMemo(() => {
    const counts = new Map<string, number>();
    for (const clip of clips) {
      for (const tag of clip.tags) counts.set(tag, (counts.get(tag) ?? 0) + 1);
    }
    return [...counts]
      .map(([name, count]) => ({ name, count }))
      .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name, "en"));
  }, [clips]);

  const untagged = useMemo(() => clips.filter((c) => !c.game).length, [clips]);
  const favorites = useMemo(() => clips.filter((c) => c.favorite).length, [clips]);
  const kinds = useMemo(() => kindCounts(clips), [clips]);

  // If a clip's game is renamed or removed, its chip disappears and so must
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
  const tiles = usePresence(visible, (clip) => clip.id, LEAVE_MS, (id) => !clipIds.has(id));

  /** The tiles under their day headings — one group without a heading for the
      sorts that have no days. */
  const groups = groupByDay(tiles, (tile) => tile.item.createdAt, sort, Date.now());

  // The grid is one tab stop: only the tile the focus last rested on is
  // reachable with Tab, the arrow keys move between the tiles.
  const [resting, setResting] = useState<string | null>(null);
  const restingId = resting !== null && order.includes(resting) ? resting : (order[0] ?? null);
  const restingRef = useRef(restingId);
  restingRef.current = restingId;
  useLayoutEffect(() => {
    const node = grid.current;
    if (!node) return;
    applyRoving(node, restingId);
    // Tools that appear later (a question, the selection) are tiles' own
    // buttons again.
    const watch = new MutationObserver(() => applyRoving(node, restingId));
    watch.observe(node, { childList: true, subtree: true });
    return () => watch.disconnect();
  });
  const onGridKeyDown = (event: React.KeyboardEvent) => {
    const target = event.target as HTMLElement;
    if (!target.matches("[data-tile-open]")) return;
    const id = target.closest<HTMLElement>("[data-flip]")?.dataset.flip;
    if (!id) return;
    if (event.key === " ") {
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
  /** Where it stands among all tiles — the arrival is staggered by it. */
  const slots = new Map(tiles.map((tile, slot) => [tile.key, slot]));

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
    setStartAt(undefined);
    setPlaylist(visible.map((clip) => clip.id));
    touched.current = new Set(visible[index] ? [visible[index].id] : []);
    coming.current = visible[index]?.id ?? null;
    setOpen(index);
  };

  // A clip handed over from the console. The filter has to go first: the clip
  // may not be in whatever was on screen, and the player counts in the list it
  // is given. The gallery then shows that clip's type, in the current order.
  useEffect(() => {
    if (!focus) return;
    const clip = clips.find((c) => c.id === focus.id);
    if (!clip) return;
    const next: ClipFilter = { ...NO_FILTER, kind: kindOf(clip) };
    setFilter(next);
    const list = sortClips(filterClips(clips, next), useGallery.getState().sort);
    setPlaylist(list.map((c) => c.id));
    // Wie beim Öffnen aus der Galerie: was angesehen wurde, wird beim Schließen
    // in seinen Spielordner einsortiert. Ohne diese Zeile wäre der Weg über die
    // Konsole der einzige, bei dem die Datei liegen bliebe.
    touched.current = new Set([focus.id]);
    coming.current = focus.id;
    // Vor dem Öffnen, damit der Player die Stelle schon beim ersten Bild hat.
    setStartAt(focus.at);
    setOpen(list.findIndex((c) => c.id === focus.id));
    onFocused?.();
  }, [focus, clips, onFocused]);

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
          games={games}
          tags={tags}
          untagged={untagged}
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
          {groups.map((group) => (
            <section key={group.key}>
              {group.label && (
                <DayHeading
                  label={group.label}
                  count={group.items.filter((tile) => !tile.leaving).length}
                />
              )}
              <div className="grid grid-cols-3 gap-4">
                {group.items.map(({ item: clip, key, leaving }) => {
                  const index = positions.get(clip.id) ?? 0;
                  const slot = slots.get(key) ?? 0;
                  // Capped, so a gallery of eighty clips does not crawl in.
                  const wait = greeting.current?.has(key) ? Math.min(slot, 11) * 30 : null;
                  return (
                    <ClipCard
                      key={key}
                      clip={clip}
                      flipKey={key}
                      leaving={leaving}
                      arriving={wait !== null || arrived.current.has(key)}
                      delay={wait}
                      link={links[clip.id]}
                      asking={asking?.id === clip.id ? asking.what : null}
                      onAsk={(what) => setAsking(what ? { id: clip.id, what } : null)}
                      renaming={renaming === clip.id}
                      onRenaming={(on) => setRenaming(on ? clip.id : null)}
                      thumbRef={(node) => {
                        if (node) thumbs.current.set(clip.id, node);
                        else thumbs.current.delete(clip.id);
                      }}
                      selected={selection.selected.has(clip.id)}
                      selecting={selecting}
                      onSelect={(mods) => selection.select(clip.id, mods)}
                      onOpen={() => openAt(index)}
                      onExport={() => setExporting(clip)}
                      onFavorite={() => void toggleFavorite(clip)}
                      onDelete={() => void deleteClip(clip.id)}
                      onDiscard={() =>
                        void (clip.originalAvailable ? discardClipOriginal(clip.id) : bakeClipCam(clip.id))
                      }
                      onContextMenu={(event) => {
                        // If the cursor is in the name field the menu belongs to the
                        // text — `TextMenu` takes care of that on its own.
                        if (isTextField(event.target)) return;
                        clipMenu(event, clip, {
                          onOpen: () => openAt(index),
                          onRename: () => setRenaming(clip.id),
                          onDelete: () => setAsking({ id: clip.id, what: "delete" }),
                          onDiscardOriginal: () => setAsking({ id: clip.id, what: "discard" }),
                          onExport: () => setExporting(clip),
                        });
                      }}
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
            for (const clip of clips) await deleteClip(clip.id);
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
function DayHeading({ label, count }: { label: string; count: number }) {
  return (
    <h2 className="mb-3 flex items-baseline gap-2.5 text-[15px] font-semibold text-ink/80">
      {label}
      <span className="text-[13px] font-normal text-ink-faint tabular-nums">
        {count === 1 ? "1 clip" : `${count} clips`}
      </span>
    </h2>
  );
}

/** What a tile is asking about, in its own place over the thumbnail. */
type Asking = "delete" | "discard";

/**
 * One tile of the gallery: thumbnail with its pills, the heart, folder and bin
 * on hover, the name to rename in place, and Open and Share.
 *
 * Everything it changes goes through its props, so the gallery stays the one
 * place that knows which tile is asking, renaming or open.
 */
function ClipCard({
  clip,
  flipKey,
  leaving,
  arriving,
  delay,
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
}: {
  clip: Clip;
  /** The tile's key for `useFlip` — the presence key, the clip id. */
  flipKey: string;
  leaving: boolean;
  /** Plays the arrival — the first greeting or a clip saved just now. */
  arriving: boolean;
  /** How long the arrival waits, for the staggered greeting. */
  delay: number | null;
  link: Link | undefined;
  asking: Asking | null;
  onAsk: (what: Asking | null) => void;
  renaming: boolean;
  onRenaming: (on: boolean) => void;
  thumbRef: (node: HTMLButtonElement | null) => void;
  /** In the selection — a ring and a filled check. */
  selected: boolean;
  /** Something is selected somewhere: a click picks instead of opening. */
  selecting: boolean;
  /** Ctrl-click and Shift-click anywhere on the tile, any click on the picture
   *  while a selection exists. */
  onSelect: (mods: SelectMods) => void;
  onOpen: () => void;
  onExport: () => void;
  onFavorite: () => void;
  onDelete: () => void;
  onDiscard: () => void;
  onContextMenu: (event: React.MouseEvent) => void;
}) {
  return (
    <Card
      data-flip={flipKey}
      interactive={!leaving}
      className={cn(
        "group overflow-hidden",
        leaving ? "cb-tile-out" : arriving && "cb-tile-in",
        selected && "border-accent shadow-[0_0_0_1px_var(--color-accent)] hover:border-accent",
      )}
      style={delay !== null ? { animationDelay: `${delay}ms` } : undefined}
      onContextMenu={onContextMenu}
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
        event.preventDefault();
        event.stopPropagation();
        onSelect({ shift: event.shiftKey, toggle: true });
      }}
      // Shift-click would start a text selection across the tiles.
      onMouseDown={(event) => {
        if (event.shiftKey && !isTextField(event.target)) event.preventDefault();
      }}
    >
      <div className="relative">
        <button
          ref={thumbRef}
          data-tile-open
          data-ring-inset
          onClick={onOpen}
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
          onClick={onFavorite}
          className={cn(
            "absolute top-3 left-3 grid h-8 w-8 place-items-center rounded-pill",
            "bg-black/50 backdrop-blur-md transition",
            clip.favorite
              ? "text-live"
              : "text-white/70 opacity-0 group-hover:opacity-100 group-has-[:focus-visible]:opacity-100 hover:text-white",
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
          className={cn(
            "absolute top-3 right-3 flex gap-1.5 transition-opacity",
            selecting && "hidden",
            // While the question stands it has to stay up, even once
            // the cursor has wandered off the tile.
            asking
              ? "opacity-100"
              : "opacity-0 group-hover:opacity-100 group-has-[:focus-visible]:opacity-100",
          )}
        >
          {asking === "delete" ? (
            <ConfirmDelete
              origin="right"
              // Its share link goes along — worth saying before, not after.
              question={link ? "Delete with its link?" : undefined}
              onConfirm={() => {
                onAsk(null);
                onDelete();
              }}
              onCancel={() => onAsk(null)}
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
                onAsk(null);
                onDiscard();
              }}
              onCancel={() => onAsk(null)}
            />
          ) : (
            <>
              <IconAction label="Show in folder" onClick={() => inTauri && api.revealClip(clip.id)}>
                <IconFolder className="h-4 w-4" />
              </IconAction>
              <IconAction label="Delete clip" danger onClick={() => onAsk("delete")}>
                <IconTrash className="h-4 w-4" />
              </IconAction>
            </>
          )}
        </div>
      </div>
      <div className="p-4">
        <NameField clip={clip} editing={renaming} onEditing={onRenaming} />
        <p className="mt-1 truncate text-xs text-ink-muted">
          {clip.game ? `${clip.game} · ` : ""}
          {formatAgo(clip.createdAt)} ·{" "}
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
          <Button size="sm" onClick={onOpen}>
            Open
          </Button>
          <ShareMenuButton clip={clip} variant="secondary" onExport={onExport} />
        </div>
      </div>
    </Card>
  );
}

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
      style={{ background: "var(--glass)", backdropFilter: "blur(20px)" }}
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
          onClick={() => pickFriend(clips[0], clips.slice(1))}
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
          { alignRight: true },
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

/** At most this many lines of chips — whatever does not fit goes into "+N more". */
const CHIP_LINES = 2;
/** `gap-2` of the row, in pixels. */
const CHIP_GAP = 8;

/** One toggle in the chip row. */
interface ChipSpec {
  /** Prefixed by kind, so a game called "Favorites" stays apart from the chip. */
  id: string;
  label: string;
  /** How it reads in the "+N more" menu, where there is no room for an icon. */
  menuLabel: string;
  count: number;
  selected: boolean;
  icon?: (selected: boolean) => React.ReactNode;
  /** The filter with this chip on or off. */
  set: (filter: ClipFilter, on: boolean) => ClipFilter;
  /** Games only: the name the hover-× would detach. */
  game?: string;
}

/**
 * The type segment and the chips that combine with it.
 *
 * The chips wrap onto at most two lines; what does not fit collapses into a
 * last chip "+N more" with a menu, and chosen chips are the last to go there.
 * Recently deleted sits at the end of the row, or at the end of that menu once
 * it exists. A game chip keeps its hover-× that detaches the name from its
 * clips — on a chosen chip the × means "off" instead, and the whole chip is
 * the button for it.
 */
function FilterBar({
  games,
  tags,
  untagged,
  favorites,
  kinds,
  deleted,
  filter,
  inTrash,
  onChange,
  onTrash,
  onRemove,
}: {
  games: Array<{ name: string; count: number }>;
  tags: Array<{ name: string; count: number }>;
  untagged: number;
  favorites: number;
  kinds: Record<ClipKind, number>;
  deleted: number;
  filter: ClipFilter;
  /** Recently deleted is open: nothing in the segment or the chips is lit. */
  inTrash: boolean;
  onChange: (filter: ClipFilter) => void;
  onTrash: () => void;
  onRemove: (game: string) => void;
}) {
  const menu = useMenu();
  const row = useRef<HTMLDivElement>(null);
  const lead = useRef<HTMLDivElement>(null);
  const mirror = useRef<HTMLDivElement>(null);
  const [confirming, setConfirming] = useState<string | null>(null);
  const [fit, setFit] = useState<{ hidden: string[]; overflow: boolean }>({
    hidden: [],
    overflow: false,
  });

  const specs: ChipSpec[] = [
    ...(favorites > 0
      ? [
          {
            id: "fav",
            label: "Favorites",
            menuLabel: "Favorites",
            count: favorites,
            selected: filter.favorites,
            icon: (on: boolean) => (
              <IconHeart filled className={cn("h-3.5 w-3.5", on ? "text-black/70" : "text-live")} />
            ),
            set: (f: ClipFilter, on: boolean) => ({ ...f, favorites: on }),
          },
        ]
      : []),
    ...games.map(({ name, count }) => ({
      id: `game:${name}`,
      label: name,
      menuLabel: name,
      count,
      selected: filter.games.includes(name),
      set: (f: ClipFilter, on: boolean) => ({
        ...f,
        games: on === f.games.includes(name) ? f.games : toggled(f.games, name),
      }),
      game: name,
    })),
    ...(untagged > 0
      ? [
          {
            id: "nogame",
            label: "No game",
            menuLabel: "No game",
            count: untagged,
            selected: filter.noGame,
            set: (f: ClipFilter, on: boolean) => ({ ...f, noGame: on }),
          },
        ]
      : []),
    // Tags come after the games: a game every clip has, a tag only some.
    ...tags.map(({ name, count }) => ({
      id: `tag:${name}`,
      label: name,
      menuLabel: `#${name}`,
      count,
      selected: filter.tags.includes(name),
      icon: (on: boolean) => <span className={on ? "text-black/45" : "text-ink-faint"}>#</span>,
      set: (f: ClipFilter, on: boolean) => ({
        ...f,
        tags: on === f.tags.includes(name) ? f.tags : toggled(f.tags, name),
      }),
    })),
  ];
  // Recently deleted shows a list of its own; the filters wait behind it,
  // unlit, and a chip picked from there brings the gallery back with it on.
  const lit = (spec: ChipSpec) => !inTrash && spec.selected;
  const pick = (spec: ChipSpec) => onChange(spec.set(filter, inTrash || !spec.selected));

  // Everything a chip's width depends on — the widths are measured again
  // whenever it changes, and when the row itself changes size.
  const signature = [
    ...specs.map((s) => `${s.id}:${s.count}:${lit(s)}:${confirming === s.game}`),
    `trash:${deleted}:${inTrash}`,
  ].join("|");
  const latest = useRef({ specs, lit });
  latest.current = { specs, lit };

  useLayoutEffect(() => {
    const measure = () => {
      const box = mirror.current;
      if (!row.current || !lead.current || !box) return;
      const width = (id: string) =>
        box.querySelector<HTMLElement>(`[data-measure="${CSS.escape(id)}"]`)?.offsetWidth ?? 0;
      const next = fitChips({
        lead: lead.current.offsetWidth,
        chips: latest.current.specs.map((s) => ({
          id: s.id,
          width: width(s.id),
          selected: latest.current.lit(s),
        })),
        tail: width("trash"),
        more: width("more"),
        row: row.current.clientWidth,
        gap: CHIP_GAP,
        lines: CHIP_LINES,
      });
      // With nothing left to hide there is no "+0 more": the trash stays a button.
      const settled = next.hidden.length > 0 ? next : { hidden: [], overflow: false };
      setFit((was) =>
        was.overflow === settled.overflow && was.hidden.join("|") === settled.hidden.join("|")
          ? was
          : settled,
      );
    };
    measure();
    const observer = new ResizeObserver(measure);
    if (row.current) observer.observe(row.current);
    if (mirror.current) observer.observe(mirror.current);
    return () => observer.disconnect();
  }, [signature]);

  // The confirmation must not stay up when you carry on elsewhere.
  useEffect(() => {
    if (!confirming) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setConfirming(null);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [confirming]);

  const hidden = new Set(fit.hidden);
  const shown = specs.filter((s) => !hidden.has(s.id));
  const overflow = fit.overflow && fit.hidden.length > 0;

  // Chips are sorted by how often a game turns up, and the confirmation takes
  // the place of the chip it belongs to. Both shift everything to the right of
  // them, and that should be a slide rather than a jump.
  useFlip(
    row,
    [
      ...shown.map((s) => (confirming !== null && confirming === s.game ? `${s.id}?` : s.id)),
      overflow ? "more" : deleted > 0 && "trash",
    ]
      .filter(Boolean)
      .join(),
  );

  const chipFor = (spec: ChipSpec, measuring = false) =>
    spec.game !== undefined && confirming === spec.game ? (
      <ConfirmChip
        key={spec.id}
        id={spec.id}
        name={spec.label}
        autoFocus={!measuring}
        onConfirm={() => {
          onRemove(spec.label);
          setConfirming(null);
        }}
        onCancel={() => setConfirming(null)}
      />
    ) : (
      <Chip
        key={spec.id}
        id={spec.id}
        active={lit(spec)}
        count={spec.count}
        icon={spec.icon?.(lit(spec))}
        onClick={() => pick(spec)}
        onRemove={spec.game !== undefined ? () => setConfirming(spec.game ?? null) : undefined}
      >
        {spec.label}
      </Chip>
    );

  const trashButton = (
    <Button
      variant={inTrash ? "primary" : "ghost"}
      size="sm"
      aria-pressed={inTrash}
      icon={<IconTrash className="h-3.5 w-3.5" />}
      onClick={onTrash}
    >
      Recently deleted
      <span className={cn("tabular-nums", inTrash ? "text-black/45" : "text-ink-faint")}>
        {deleted}
      </span>
    </Button>
  );

  const openMore = (event: React.MouseEvent<HTMLButtonElement>) => {
    const box = event.currentTarget.getBoundingClientRect();
    const entries: MenuEntry[] = specs
      .filter((s) => hidden.has(s.id))
      .map((s) => ({
        kind: "item",
        label: s.menuLabel,
        icon: lit(s) ? <IconCheck className="h-3.5 w-3.5" /> : undefined,
        shortcut: String(s.count),
        onSelect: () => pick(s),
      }));
    if (deleted > 0) {
      entries.push(
        { kind: "separator" },
        {
          kind: "item",
          label: "Recently deleted",
          icon: inTrash ? <IconCheck className="h-3.5 w-3.5" /> : <IconTrash className="h-3.5 w-3.5" />,
          shortcut: String(deleted),
          onSelect: onTrash,
        },
      );
    }
    menu.open(
      {
        clientX: box.left,
        clientY: box.bottom + 6,
        preventDefault: () => {},
        stopPropagation: () => {},
      },
      entries,
    );
  };

  return (
    <div className="relative">
      {/* Every chip once more, out of sight and in one line: this is where
          their widths come from. Clipped to nothing, so it cannot widen the
          page. */}
      <div aria-hidden inert className="invisible absolute inset-x-0 top-0 h-0 overflow-hidden">
        <div ref={mirror} className="flex w-max items-center gap-2">
          {specs.map((spec) => (
            <div key={spec.id} data-measure={spec.id} className="shrink-0">
              {chipFor(spec, true)}
            </div>
          ))}
          {deleted > 0 && (
            <div data-measure="trash" className="shrink-0">
              {trashButton}
            </div>
          )}
          <div data-measure="more" className="shrink-0">
            <MoreChip count={88} />
          </div>
        </div>
      </div>

      <div ref={row} className="flex flex-wrap items-center gap-2 py-0.5">
        <div ref={lead} className="pr-1.5">
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
        {shown.map((spec) => chipFor(spec))}
        {overflow ? (
          <MoreChip count={fit.hidden.length} onClick={openMore} />
        ) : (
          deleted > 0 && (
            <div data-flip="trash" className="shrink-0">
              {trashButton}
            </div>
          )
        )}
      </div>
    </div>
  );
}

/** The last chip of a full row: the ones that did not fit, as a menu. */
function MoreChip({
  count,
  onClick,
}: {
  count: number;
  onClick?: (event: React.MouseEvent<HTMLButtonElement>) => void;
}) {
  return (
    <button
      data-flip="more"
      aria-haspopup="menu"
      onClick={onClick}
      className="flex h-8 shrink-0 items-center rounded-pill border border-dashed border-line bg-surface
        px-4 text-[13px] font-medium text-ink-muted transition-colors duration-150
        hover:border-line-strong hover:text-ink"
    >
      +{count} more
    </button>
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

/**
 * One filter pill, on or off. A chosen one is white with a × — the whole pill is
 * the button that switches it off again.
 *
 * A game's pill also has the × that detaches the name from its clips, only
 * while it is not chosen, so the two never sit in the same place at once. It
 * holds its place in the layout and only becomes visible on hover — if it did
 * not, every following pill would jump sideways as you moved across. The ×
 * of a chosen pill takes exactly that place.
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
        aria-pressed={active}
        title={active ? `${children} — click to turn off` : children}
        className={cn(
          "flex h-full min-w-0 items-center gap-1.5 rounded-pill pl-4 text-[13px] font-medium",
          active ? "pr-1" : onRemove ? "pr-1.5" : "pr-4",
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
      {onRemove && !active && (
        <button
          aria-label={`Remove the "${children}" filter`}
          title="Remove filter — the game name is detached from these clips"
          onClick={onRemove}
          className={cn(
            "mr-1 grid h-6 w-6 shrink-0 place-items-center rounded-pill opacity-0 transition",
            "group-hover/chip:opacity-100 focus-visible:opacity-100 hover:bg-hover hover:text-live",
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
  autoFocus = true,
  onConfirm,
  onCancel,
}: {
  id: string;
  name: string;
  /** Off for the copy that is only there to be measured. */
  autoFocus?: boolean;
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
        autoFocus={autoFocus}
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

/** What "Free up space" costs on this clip, said before it happens. */
function freeSpaceTitle(clip: Clip): string {
  const cam = clip.facecam?.state === "ready";
  if (!clip.originalAvailable) return "The cam can no longer be moved in this clip.";
  return cam
    ? "The trim stays, undo goes. The cam can no longer be moved in this clip."
    : "The trim stays, undo goes";
}
