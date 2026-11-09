/**
 * Which control a controller step lands on — the pure part of the console's
 * navigation (`src/console/useNav.ts`). Boxes in, an index out, so it can be
 * tested without a page.
 *
 * Spatial rather than a list per row: the console's three dock shapes (bar,
 * pill, ring), the clip grid with its tool rows, the player and the friend
 * offers at the top all lie differently, and "the nearest thing that way"
 * fits each of them without knowing it.
 */

export type Dir = "up" | "down" | "left" | "right";

export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** How much a step sideways of the way pressed counts against a candidate,
 *  against one straight ahead. High, so a row is followed to its end before
 *  the ring jumps to the next one. */
const SIDEWAYS = 3;

/** Distance between two ranges on one axis; 0 when they overlap. */
function gap(a0: number, a1: number, b0: number, b1: number): number {
  if (b0 >= a1) return b0 - a1;
  if (a0 >= b1) return a0 - b1;
  return 0;
}

/**
 * The box nearest to `from` in direction `dir`, or -1 when there is none.
 *
 * A candidate counts as lying that way when its centre is past `from`'s
 * centre and its near edge past `from`'s near edge. It is scored by the gap ahead, plus the gap sideways (zero while
 * the two overlap on that axis) weighed by {@link SIDEWAYS}; ties go to the
 * centre closer sideways, then to the earlier box.
 */
export function pickNext(from: Box, boxes: Box[], dir: Dir): number {
  const fx = from.left + from.width / 2;
  const fy = from.top + from.height / 2;
  const horizontal = dir === "left" || dir === "right";
  const sign = dir === "right" || dir === "down" ? 1 : -1;
  let best = -1;
  let bestScore = Infinity;
  let bestTie = Infinity;
  boxes.forEach((box, index) => {
    const cx = box.left + box.width / 2;
    const cy = box.top + box.height / 2;
    const ahead = horizontal ? (cx - fx) * sign : (cy - fy) * sign;
    if (ahead <= 1) return;
    // Its near edge past `from`'s too: a tile whose centre lies a little
    // right of a small tool under its left end is not "right" of that tool.
    const beyond =
      dir === "right"
        ? box.left > from.left
        : dir === "left"
          ? box.left + box.width < from.left + from.width
          : dir === "down"
            ? box.top > from.top
            : box.top + box.height < from.top + from.height;
    if (!beyond) return;
    const forward = horizontal
      ? sign > 0
        ? Math.max(0, box.left - (from.left + from.width))
        : Math.max(0, from.left - (box.left + box.width))
      : sign > 0
        ? Math.max(0, box.top - (from.top + from.height))
        : Math.max(0, from.top - (box.top + box.height));
    const sideways = horizontal
      ? gap(from.top, from.top + from.height, box.top, box.top + box.height)
      : gap(from.left, from.left + from.width, box.left, box.left + box.width);
    const score = forward + sideways * SIDEWAYS;
    const tie = horizontal ? Math.abs(cy - fy) : Math.abs(cx - fx);
    if (score < bestScore || (score === bestScore && tie < bestTie)) {
      best = index;
      bestScore = score;
      bestTie = tie;
    }
  });
  return best;
}

/** The box whose centre lies nearest to `at`'s — where the ring goes when
 *  the control it stood on is gone and nothing remembers where it came from.
 *  -1 for no boxes. */
export function nearest(at: Box, boxes: Box[]): number {
  const ax = at.left + at.width / 2;
  const ay = at.top + at.height / 2;
  let best = -1;
  let bestDistance = Infinity;
  boxes.forEach((box, index) => {
    const dx = box.left + box.width / 2 - ax;
    const dy = box.top + box.height / 2 - ay;
    const distance = dx * dx + dy * dy;
    if (distance < bestDistance) {
      best = index;
      bestDistance = distance;
    }
  });
  return best;
}

/** How many steps of a range input one press moves: about forty presses from
 *  end to end, at least one step. */
export function rangeSteps(min: number, max: number, step: number): number {
  if (!(step > 0) || !(max > min)) return 1;
  return Math.max(1, Math.round((max - min) / step / 40));
}

/** The tab after (`+1`) or before (`-1`) the open one, wrapping round. With
 *  none open, the first going forward and the last going back. -1 for no
 *  tabs. */
export function nextTab(count: number, open: number, by: 1 | -1): number {
  if (count <= 0) return -1;
  if (open < 0) return by > 0 ? 0 : count - 1;
  return (((open + by) % count) + count) % count;
}
