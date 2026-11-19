/**
 * Selecting several clips in the gallery — the rules, apart from the screen.
 *
 * Every function takes the selection as a set of clip ids and hands back a new
 * one; the order they work in is the gallery as it is shown, so a Shift-range
 * follows what the eye sees, across day headings.
 */

/** Ctrl-click: this one in or out, the rest as it was. */
export function toggleId(selected: ReadonlySet<string>, id: string): Set<string> {
  const next = new Set(selected);
  if (!next.delete(id)) next.add(id);
  return next;
}

/**
 * Shift-click: everything from the last clicked card to this one, added to what
 * was selected. Without an anchor (nothing clicked yet, or it has left the
 * list) the target alone is selected. An unknown target changes nothing.
 */
export function rangeSelect(
  order: readonly string[],
  anchor: string | null,
  target: string,
  selected: ReadonlySet<string>,
): Set<string> {
  const to = order.indexOf(target);
  if (to === -1) return selected as Set<string>;
  const from = anchor === null ? -1 : order.indexOf(anchor);
  const next = new Set(selected);
  if (from === -1) {
    next.add(target);
    return next;
  }
  for (const id of order.slice(Math.min(from, to), Math.max(from, to) + 1)) next.add(id);
  return next;
}

/** Ctrl+A: all that is shown. */
export function selectAll(order: readonly string[]): Set<string> {
  return new Set(order);
}

/**
 * Drop what is no longer in the list — hidden by a filter, deleted elsewhere.
 * The very same set comes back when nothing dropped, so a state update with it
 * does not render again.
 */
export function pruneSelection(selected: ReadonlySet<string>, order: readonly string[]): Set<string> {
  const shown = new Set(order);
  let dropped = false;
  const next = new Set<string>();
  for (const id of selected) {
    if (shown.has(id)) next.add(id);
    else dropped = true;
  }
  return dropped ? next : (selected as Set<string>);
}

/** The Favorite button of the selection bar: if every clip has a heart it takes
 *  them away, otherwise it gives one to those without. */
export function favoriteTarget(clips: ReadonlyArray<{ favorite: boolean }>): boolean {
  return !clips.every((clip) => clip.favorite);
}
