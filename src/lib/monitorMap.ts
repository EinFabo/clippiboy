import type { CaptureTarget, TargetRect } from "./types.ts";

/** A screen on the virtual desktop, physical pixels. */
export interface DeskRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Where a screen is drawn inside the map, in CSS pixels. */
export interface MapBox {
  left: number;
  top: number;
  width: number;
  height: number;
}

/**
 * The screens as Windows arranges them, scaled down into `area` and centred.
 *
 * Every screen keeps its aspect and its size against the others; a screen left
 * of or above the primary one has negative coordinates, which only move the
 * bounding box. `gap` is taken off between screens that touch, half from each
 * side, so two of them side by side do not read as one wide panel.
 */
export function layoutMonitors(
  rects: DeskRect[],
  area: { width: number; height: number },
  gap = 6,
): MapBox[] {
  if (rects.length === 0 || area.width <= 0 || area.height <= 0) return [];
  const minX = Math.min(...rects.map((r) => r.x));
  const minY = Math.min(...rects.map((r) => r.y));
  const maxX = Math.max(...rects.map((r) => r.x + r.width));
  const maxY = Math.max(...rects.map((r) => r.y + r.height));
  const spanX = Math.max(maxX - minX, 1);
  const spanY = Math.max(maxY - minY, 1);
  const scale = Math.min(area.width / spanX, area.height / spanY);
  const offX = (area.width - spanX * scale) / 2;
  const offY = (area.height - spanY * scale) / 2;
  const inset = gap / 2;
  return rects.map((r) => {
    const width = Math.max(r.width * scale - gap, 1);
    const height = Math.max(r.height * scale - gap, 1);
    return {
      left: round(offX + (r.x - minX) * scale + inset),
      top: round(offY + (r.y - minY) * scale + inset),
      width: round(width),
      height: round(height),
    };
  });
}

const round = (n: number) => Math.round(n * 10) / 10;

/** A monitor's place as the config keeps it; `null` for a window. */
export function rectOf(t: CaptureTarget): TargetRect | null {
  return t.kind === "monitor" ? { x: t.x, y: t.y, width: t.width, height: t.height } : null;
}

export function sameRect(a: TargetRect | null, b: TargetRect | null): boolean {
  return a === b || (!!a && !!b && a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height);
}

/**
 * The screens in the order they are numbered: the primary one is 1, the rest
 * follow as Windows lists them.
 */
export function numberedMonitors(targets: CaptureTarget[]): CaptureTarget[] {
  const monitors = targets.filter((t) => t.kind === "monitor");
  return [...monitors.filter((t) => t.isPrimary), ...monitors.filter((t) => !t.isPrimary)];
}

/** The windows, the detected game on top. */
export function orderedWindows(targets: CaptureTarget[]): CaptureTarget[] {
  const windows = targets.filter((t) => t.kind === "window");
  return [...windows.filter((t) => t.isGame), ...windows.filter((t) => !t.isGame)];
}
