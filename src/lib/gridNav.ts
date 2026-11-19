/** The keys that move between tiles. */
export type NavKey = "ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown" | "Home" | "End";

export const NAV_KEYS: readonly string[] = [
  "ArrowLeft",
  "ArrowRight",
  "ArrowUp",
  "ArrowDown",
  "Home",
  "End",
];

/**
 * Where an arrow key takes the focus in a gallery cut into day sections of
 * `columns` tiles per row. Left and Right walk the flat order, Up and Down keep
 * the column within a section and cross into the neighbouring section's
 * nearest row; a row that is too short gives its last tile. Stays put at the
 * edges. `null` when the tile is not in the grid.
 */
export function moveInGrid(
  sections: readonly (readonly string[])[],
  columns: number,
  from: string,
  key: NavKey,
): string | null {
  const flat = sections.flat();
  if (flat.length === 0) return null;
  const at = flat.indexOf(from);
  if (at < 0) return null;
  if (key === "Home") return flat[0];
  if (key === "End") return flat[flat.length - 1];
  if (key === "ArrowLeft") return flat[Math.max(0, at - 1)];
  if (key === "ArrowRight") return flat[Math.min(flat.length - 1, at + 1)];

  // Section and place within it.
  let section = 0;
  let offset = at;
  while (offset >= sections[section].length) offset -= sections[section++].length;
  const row = Math.floor(offset / columns);
  const column = offset % columns;
  const rows = (s: number) => Math.ceil(sections[s].length / columns);
  /** The tile in a row of a section at this column, or the row's last. */
  const pick = (s: number, r: number) =>
    sections[s][Math.min(r * columns + column, sections[s].length - 1)];

  if (key === "ArrowDown") {
    if (row + 1 < rows(section)) return pick(section, row + 1);
    let next = section + 1;
    while (next < sections.length && sections[next].length === 0) next++;
    return next < sections.length ? pick(next, 0) : from;
  }
  if (row > 0) return pick(section, row - 1);
  let prev = section - 1;
  while (prev >= 0 && sections[prev].length === 0) prev--;
  return prev >= 0 ? pick(prev, rows(prev) - 1) : from;
}
