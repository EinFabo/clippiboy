import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { pruneSelection, rangeSelect, selectAll, toggleId } from "./clipSelect";

/** What of a click decides how it selects. */
export interface SelectMods {
  shift?: boolean;
  /** Ctrl, or Cmd on a Mac. */
  toggle?: boolean;
}

/**
 * The gallery's selection: which clips, and the card the last click landed on.
 *
 * `order` is the gallery as shown (sorted, filtered). Whatever drops out of it
 * drops out of the selection too. Keyboard code can call the same functions as
 * the mouse: `toggle(id)` for Space, `select(id, { shift: true })` for
 * Shift+Arrow, `all()`, `clear()`.
 */
export function useClipSelection(order: readonly string[]) {
  const [selected, setSelected] = useState<ReadonlySet<string>>(() => new Set());
  const anchor = useRef<string | null>(null);

  // Hidden by a filter or the search, deleted, or Recently deleted opened.
  useEffect(() => {
    setSelected((current) => pruneSelection(current, order));
  }, [order]);

  /** A click on a card: Shift extends from the last one, otherwise it toggles. */
  const select = useCallback(
    (id: string, mods: SelectMods = {}) => {
      // Read now: the updater below runs later, after the anchor has moved on.
      const from = anchor.current;
      setSelected((current) =>
        mods.shift ? rangeSelect(order, from, id, current) : toggleId(current, id),
      );
      anchor.current = id;
    },
    [order],
  );
  const toggle = useCallback((id: string) => select(id), [select]);
  const all = useCallback(() => setSelected(selectAll(order)), [order]);
  const clear = useCallback(() => {
    setSelected((current) => (current.size === 0 ? current : new Set()));
    anchor.current = null;
  }, []);

  return useMemo(
    () => ({ selected, count: selected.size, select, toggle, all, clear }),
    [selected, select, toggle, all, clear],
  );
}
