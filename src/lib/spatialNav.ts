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

/**
 * A control the ring can stand on, for {@link navigate}. Controls inside a
 * group (`data-nav-rows`, the player) are laid out by the page in rows
 * (`data-nav-row`), and there the ring follows the rows rather than the
 * geometry — see {@link pickInRows}.
 */
export interface Stop {
  box: Box;
  /** Which group it belongs to, if any. */
  group?: number;
  /** Its row inside the group, counted from the top. */
  row?: number;
}

/**
 * The next control in a group laid out in rows: left and right go round the
 * row (from the last back to the first, so nothing is a dead end), up and
 * down to the row above or below — onto the control whose centre is closest
 * sideways. `null` when there is no row that way: the step leaves the group.
 *
 * `rows` holds the controls of each row, top to bottom, left to right.
 */
export function pickInRows(
  rows: Box[][],
  row: number,
  col: number,
  dir: Dir,
): { row: number; col: number } | null {
  const here = rows[row];
  if (!here || !here[col]) return null;
  if (dir === "left" || dir === "right") {
    const by = dir === "right" ? 1 : -1;
    return { row, col: (((col + by) % here.length) + here.length) % here.length };
  }
  const by = dir === "down" ? 1 : -1;
  const from = here[col];
  const cx = from.left + from.width / 2;
  for (let r = row + by; r >= 0 && r < rows.length; r += by) {
    const target = rows[r];
    if (target.length === 0) continue;
    let best = 0;
    target.forEach((box, index) => {
      const d = Math.abs(box.left + box.width / 2 - cx);
      const b = target[best];
      if (d < Math.abs(b.left + b.width / 2 - cx)) best = index;
    });
    return { row: r, col: best };
  }
  return null;
}

/**
 * Where a step from `stops[from]` lands, or -1. Inside a group by its rows;
 * leaving a group (up or down past its edge), and everywhere else, by
 * geometry ({@link pickNext}) over the controls outside that group — and
 * into a group through its near row.
 */
export function navigate(stops: Stop[], from: number, dir: Dir): number {
  const start = stops[from];
  if (!start) return -1;
  if (start.group !== undefined && start.row !== undefined) {
    const group = start.group;
    const rowIds = [...new Set(stops.filter((s) => s.group === group).map((s) => s.row ?? 0))].sort(
      (a, b) => a - b,
    );
    const rows = rowIds.map((id) => stops.map((s, i) => ({ s, i })).filter(({ s }) => s.group === group && s.row === id));
    const r = rowIds.indexOf(start.row);
    const c = rows[r].findIndex(({ i }) => i === from);
    const hit = pickInRows(
      rows.map((list) => list.map(({ s }) => s.box)),
      r,
      c,
      dir,
    );
    if (hit) return rows[hit.row][hit.col].i;
    const outside = stops.map((s, i) => ({ s, i })).filter(({ s }) => s.group !== group);
    const at = pickNext(start.box, outside.map(({ s }) => s.box), dir);
    return at >= 0 ? enter(stops, outside[at].i, start.box, dir) : -1;
  }
  const others = stops.map((s, i) => ({ s, i })).filter(({ i }) => i !== from);
  const at = pickNext(start.box, others.map(({ s }) => s.box), dir);
  if (at < 0) return -1;
  return enter(stops, others[at].i, start.box, dir);
}

/**
 * Geometry picked `hit`. When that lies in a group and the step was up or
 * down, it comes in through the group's near edge instead — the bottom row
 * going up, the top row going down — onto the control closest sideways: a
 * wide control further in (the timeline) must not catch the step before the
 * row it passes.
 */
function enter(stops: Stop[], hit: number, from: Box, dir: Dir): number {
  const target = stops[hit];
  if (target.group === undefined || dir === "left" || dir === "right") return hit;
  const members = stops.map((s, i) => ({ s, i })).filter(({ s }) => s.group === target.group);
  const rows = members.map(({ s }) => s.row ?? 0);
  const edge = dir === "up" ? Math.max(...rows) : Math.min(...rows);
  const cx = from.left + from.width / 2;
  let best = hit;
  let bestD = Infinity;
  for (const { s, i } of members) {
    if ((s.row ?? 0) !== edge) continue;
    const d = Math.abs(s.box.left + s.box.width / 2 - cx);
    if (d < bestD) {
      best = i;
      bestD = d;
    }
  }
  return best;
}
