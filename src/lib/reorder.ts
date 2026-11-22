/**
 * The arithmetic behind lifting a row and moving it elsewhere in a list
 * (`useReorder`). Kept free of the DOM so it can be tested on its own: all of it
 * works on the rows' tops and heights as measured when the drag began.
 */

/**
 * Where the carried row would land: the slot among the other rows, 0 above the
 * first of them up to `mids.length` below the last.
 *
 * `mids` are the middles of the other rows, top down, without the carried one;
 * `y` is the middle of the carried card where it is shown right now. The card
 * passes a row as soon as its middle passes that row's middle — the moment the
 * two overlap by half, which is when it looks like it belongs on the other side.
 */
export function insertIndex(mids: readonly number[], y: number): number {
  let slot = 0;
  while (slot < mids.length && y > mids[slot]) slot++;
  return slot;
}

/**
 * The order after the row at `from` has been put down at slot `to` (counted the
 * way `insertIndex` counts, which is also its index in the new order). `null`
 * when nothing changed: putting a row back where it came from is not a change
 * and must not cost a config write — the core restarts the audio engine for one.
 */
export function moveTo(ids: readonly string[], from: number, to: number): string[] | null {
  if (from < 0 || from >= ids.length || to === from) return null;
  const next = ids.filter((_, index) => index !== from);
  const slot = Math.max(0, Math.min(to, next.length));
  next.splice(slot, 0, ids[from]);
  return next.every((id, index) => id === ids[index]) ? null : next;
}

/**
 * Which way the row at `index` steps aside while the row from `from` hovers
 * over slot `to`: up one place (-1), down one place (1) or not at all (0).
 * Multiplied by the carried row's height plus the gap, that is exactly where
 * the row will stand once the order is written.
 */
export function shiftOf(index: number, from: number, to: number): -1 | 0 | 1 {
  if (index === from) return 0;
  if (from < to && index > from && index <= to) return -1;
  if (to < from && index >= to && index < from) return 1;
  return 0;
}

/**
 * How far the carried row has to travel from its own place to stand in slot
 * `to`, with every other row stepped aside as `shiftOf` says. Moving down it
 * ends where the bottom of the row it passed last was; moving up it takes the
 * top of the row it passed last. Rows of different heights come out right too.
 */
export function landingShift(
  tops: readonly number[],
  heights: readonly number[],
  from: number,
  to: number,
): number {
  if (to === from) return 0;
  if (to > from) return tops[to] + heights[to] - heights[from] - tops[from];
  return tops[to] - tops[from];
}

/**
 * How many pixels to scroll this frame while the pointer is near an edge of the
 * scroll area: nothing in the middle, more the closer it gets, and full speed
 * at or beyond the edge. Negative scrolls up.
 */
export function scrollStep(
  y: number,
  top: number,
  bottom: number,
  zone = 48,
  max = 14,
): number {
  // An area too small for two zones would scroll whatever the pointer does.
  if (bottom - top <= zone * 2) return 0;
  const speed = (depth: number) => Math.round(max * Math.min(1, depth / zone));
  if (y < top + zone) return -speed(top + zone - y) || 0;
  if (y > bottom - zone) return speed(y - (bottom - zone));
  return 0;
}
