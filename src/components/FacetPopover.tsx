import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { createPortal } from "react-dom";
import { cn } from "@/lib/cn";
import { placeMenu } from "@/lib/menuPlace";
import { facetHasSearch, searchFacet, type FacetValue } from "@/lib/clipFilter";
import { Button } from "@/components/ui/Button";
import { useMenu } from "@/components/ui/Menu";
import { IconCheck, IconClose, IconSearch } from "@/components/icons";

/** Distance to the window edge, as for every menu. */
const EDGE = 8;
/** Space between the button and the list. */
const GAP = 6;

export interface FacetRow extends FacetValue {
  checked: boolean;
}

/**
 * The list behind "Game ▾" and "Tags ▾": a tick per value with its count, a
 * search field on top once the list is long, "No game" apart at the end and
 * Clear at the bottom. The Share menu's glass, placed like every menu.
 *
 * Not the shared `Menu`: that one keeps the focus where it was and its
 * entries as they were when it opened, and this list has a field to type in
 * and ticks that change while it stays up.
 */
export function FacetPopover({
  anchor,
  noun,
  values,
  noGame,
  onToggle,
  onToggleNoGame,
  onClear,
  onRemove,
  onClose,
}: {
  /** The button it hangs from — a click on it is not a click outside. */
  anchor: RefObject<HTMLElement | null>;
  /** "games" or "tags", for the search field. */
  noun: string;
  values: FacetRow[];
  /** The "No game" row, only when such clips exist. */
  noGame?: { count: number; checked: boolean } | null;
  onToggle: (name: string) => void;
  onToggleNoGame?: () => void;
  onClear: () => void;
  /** Games only: right-click on a row offers to detach the name from its clips. */
  onRemove?: (name: string) => void;
  onClose: () => void;
}) {
  const menu = useMenu();
  const box = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  /** The game whose name is about to be detached — its row asks first. */
  const [removing, setRemoving] = useState<string | null>(null);
  /** Where it hangs: the edge that touches the button stays put, so a search
   *  that shortens the list draws it in towards the button. */
  const [place, setPlace] = useState<
    { left: number; top?: number; bottom?: number; maxHeight: number; above: boolean } | null
  >(null);

  const searchable = facetHasSearch(values.length + (noGame ? 1 : 0));
  const shown = searchFacet(values, query);
  const showNoGame = noGame && searchFacet([{ name: "No game" }], query).length > 0;
  const anyChecked = values.some((v) => v.checked) || !!noGame?.checked;

  // Measured once, at its natural height, when it opens.
  useLayoutEffect(() => {
    const el = box.current;
    const button = anchor.current;
    if (!el || !button) return;
    const p = placeMenu({
      anchor: button.getBoundingClientRect(),
      size: { width: el.offsetWidth, height: el.offsetHeight },
      viewport: { width: window.innerWidth, height: window.innerHeight },
      margin: EDGE,
      gap: GAP,
      // The list belongs under its button; too long, it scrolls.
      slide: false,
    });
    const height = Math.min(el.offsetHeight, p.maxHeight);
    const above = p.placement === "above";
    setPlace({
      left: p.x,
      ...(above ? { bottom: window.innerHeight - p.y - height } : { top: p.y }),
      maxHeight: height,
      above,
    });
    // Only on opening: a resize closes it anyway.
  }, []);

  // Into the list straight away: the field when there is one, else the first row.
  useEffect(() => {
    const first = field.current ?? box.current?.querySelector<HTMLElement>("[data-facet-row]");
    first?.focus({ preventScroll: true });
  }, []);

  // Through a ref, so the listeners need not be hung up again on every render.
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const onDown = (event: PointerEvent) => {
      const target = event.target as Node;
      if (box.current?.contains(target) || anchor.current?.contains(target)) return;
      // The right-click menu of a row is part of the list.
      if (target instanceof Element && target.closest('[role="menu"]')) return;
      close.current();
    };
    const onScroll = (event: Event) => {
      if (event.target instanceof Node && box.current?.contains(event.target)) return;
      close.current();
    };
    const onAway = () => close.current();
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", onAway);
    window.addEventListener("blur", onAway);
    return () => {
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", onAway);
      window.removeEventListener("blur", onAway);
    };
  }, [anchor]);

  const stops = () => [
    ...(field.current ? [field.current] : []),
    ...(box.current?.querySelectorAll<HTMLElement>("[data-facet-row]") ?? []),
  ];
  const focusRow = (name: string | null) => {
    const rows = [...(box.current?.querySelectorAll<HTMLElement>("[data-facet-row]") ?? [])];
    (rows.find((row) => row.dataset.facetRow === name) ?? field.current ?? rows[0])?.focus();
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === "Escape") {
      // One step back at a time: the question, the search, then the list.
      event.preventDefault();
      event.stopPropagation();
      if (removing !== null) {
        const name = removing;
        setRemoving(null);
        requestAnimationFrame(() => focusRow(name));
      } else if (query) {
        setQuery("");
        field.current?.focus();
      } else {
        anchor.current?.querySelector<HTMLElement>("button")?.focus();
        onClose();
      }
      return;
    }
    if (event.key === "Tab") {
      // Out of the list and on from its button, as if it had never been open.
      anchor.current?.querySelector<HTMLElement>("button")?.focus();
      onClose();
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      const list = stops();
      if (list.length === 0) return;
      event.preventDefault();
      const at = list.indexOf(document.activeElement as HTMLElement);
      const step = event.key === "ArrowDown" ? 1 : -1;
      const next = at === -1 ? 0 : Math.max(0, Math.min(list.length - 1, at + step));
      list[next].focus();
      list[next].scrollIntoView({ block: "nearest" });
    }
  };

  const row = (name: string, count: number, checked: boolean, toggle: () => void, muted = false) =>
    removing === name && onRemove && !muted ? (
      <ConfirmRow
        key={name}
        name={name}
        onConfirm={() => {
          setRemoving(null);
          onRemove(name);
          requestAnimationFrame(() => focusRow(null));
        }}
        onCancel={() => {
          setRemoving(null);
          requestAnimationFrame(() => focusRow(name));
        }}
      />
    ) : (
      <button
        key={muted ? "\u0000no-game" : name}
        data-facet-row={muted ? "" : name}
        data-ring-inset
        role="menuitemcheckbox"
        aria-checked={checked}
        onClick={toggle}
        onContextMenu={(event) => {
          if (muted || !onRemove) {
            event.preventDefault();
            return;
          }
          menu.open(event, [
            { kind: "item", label: "Remove game name…", onSelect: () => setRemoving(name) },
          ]);
        }}
        className={cn(
          "flex h-[34px] w-full shrink-0 items-center gap-2.5 rounded-[10px] px-2.5 text-left text-[13px]",
          "font-medium transition-colors duration-100 hover:bg-hover focus-visible:bg-hover",
          checked && "bg-white/[0.08]",
        )}
      >
        <span
          aria-hidden
          className={cn(
            "grid h-4 w-4 shrink-0 place-items-center rounded-[5px] border-[1.5px]",
            checked ? "border-accent bg-accent text-white" : "border-white/25",
          )}
        >
          {checked && <IconCheck className="h-3 w-3" />}
        </span>
        <span className={cn("min-w-0 flex-1 truncate", muted && "text-ink-muted")}>{name}</span>
        <span className="shrink-0 text-xs text-ink-muted tabular-nums">{count}</span>
      </button>
    );

  // In the body: the page sits in a layer that scrolls and moves.
  return createPortal(
    <div
      ref={box}
      role="dialog"
      aria-label={`Filter by ${noun}`}
      onKeyDown={onKeyDown}
      onContextMenu={(event) => event.preventDefault()}
      style={{
        left: place?.left ?? 0,
        top: place ? place.top : 0,
        bottom: place?.bottom,
        maxHeight: place?.maxHeight,
        background: "var(--glass)",
        backdropFilter: "blur(20px)",
      }}
      className={cn(
        "fixed z-[70] flex w-[300px] flex-col rounded-inner border border-white/10 p-1.5",
        "shadow-[0_8px_32px_rgba(0,0,0,0.4)]",
        "transition-[opacity,transform] duration-100 ease-[var(--ease-out-soft)]",
        place?.above ? "origin-bottom-left" : "origin-top-left",
        place ? "scale-100 opacity-100" : "scale-[0.97] opacity-0",
      )}
    >
      {searchable && (
        <div className="relative mx-0.5 mt-0.5 mb-1.5 shrink-0">
          <IconSearch className="pointer-events-none absolute top-1/2 left-3 h-3.5 w-3.5 -translate-y-1/2 text-ink-faint" />
          <input
            ref={field}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={`Search ${noun}`}
            aria-label={`Search ${noun}`}
            className="h-[34px] w-full rounded-pill border border-line bg-surface pr-3 pl-8 text-[13px]
              outline-none transition-colors placeholder:text-ink-faint focus:border-line-strong"
          />
        </div>
      )}
      <div role="group" className="flex min-h-0 flex-1 flex-col overflow-y-auto overscroll-contain">
        {shown.map((value) => row(value.name, value.count, value.checked, () => onToggle(value.name)))}
        {showNoGame && noGame && (
          <>
            {shown.length > 0 && <div className="mx-2 my-1 shrink-0 border-t border-line" />}
            {row("No game", noGame.count, noGame.checked, () => onToggleNoGame?.(), true)}
          </>
        )}
        {shown.length === 0 && !showNoGame && (
          <p className="px-2.5 py-2 text-[13px] text-ink-faint">Nothing matches</p>
        )}
      </div>
      <div className="mt-1 flex shrink-0 justify-end border-t border-line pt-1.5">
        <Button variant="ghost" size="sm" className="h-[26px] px-2.5" disabled={!anyChecked} onClick={onClear}>
          Clear
        </Button>
      </div>
    </div>,
    document.fullscreenElement ?? document.body,
  );
}

/**
 * The question before a game name is detached from its clips, in the place of
 * its row — no dialog over the list.
 */
function ConfirmRow({
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
      className="cb-chip-expand flex h-[34px] w-full shrink-0 items-center gap-1 rounded-pill border
        border-live/40 bg-live/15 pl-3 text-[13px] font-medium text-live"
    >
      <span className="min-w-0 truncate" title={name}>
        {name}
      </span>
      <span className="mr-auto whitespace-nowrap">remove?</span>
      <button
        aria-label="Confirm removal"
        title="The game name is detached from these clips"
        onClick={onConfirm}
        autoFocus
        className="ml-1 grid h-6 w-6 shrink-0 place-items-center rounded-pill hover:bg-live/25"
      >
        <IconCheck className="h-3.5 w-3.5" />
      </button>
      <button
        aria-label="Cancel"
        onClick={onCancel}
        className="mr-1 grid h-6 w-6 shrink-0 place-items-center rounded-pill text-ink-muted hover:bg-hover hover:text-ink"
      >
        <IconClose className="h-3 w-3" />
      </button>
    </div>
  );
}
