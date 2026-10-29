// The camera's place in a frame, in pixels: the rule of `composite::cam_rect`,
// kept in step by hand, and its inverse for dragging the box.
import type { CamLayout, CamShape } from "./types";

const even = (v: number) => 2 * Math.round(v / 2);

/** Where the cam sits in a frame — `composite::cam_rect`, kept in step by hand. */
export function camBox(layout: CamLayout, width: number, height: number) {
  const circle = layout.shape === "circle";
  const m = even(0.025 * height);
  let h = Math.max(2, even(layout.size * height));
  let w = circle ? h : even((h * 16) / 9);
  const room = Math.max(0, width - 2 * m);
  if (w > room) {
    w = room - (room % 2);
    h = circle ? w : even((w * 9) / 16);
  }
  const freeX = Math.max(0, width - w - 2 * m);
  const freeY = Math.max(0, height - h - 2 * m);
  const x = Math.min(1, Math.max(0, layout.x));
  const y = Math.min(1, Math.max(0, layout.y));
  return { left: m + Math.min(even(x * freeX), freeX), top: m + Math.min(even(y * freeY), freeY), width: w, height: h };
}


const MIN_SIZE = 0.12;
const MAX_SIZE = 0.5;
/** Within this of an edge the box snaps onto it. */
const SNAP = 0.03;

/** The layout whose `camBox` is `rect` (picture pixels): the inverse of the rule above. */
export function layoutFromRect(
  rect: { x: number; y: number; width: number; height: number },
  frame: { width: number; height: number },
  shape: CamShape,
): Pick<CamLayout, "x" | "y" | "size"> {
  const size = Math.min(MAX_SIZE, Math.max(MIN_SIZE, rect.height / frame.height));
  const m = even(0.025 * frame.height);
  const { width: w, height: h } = camBox({ visible: true, x: 0, y: 0, size, shape, mirror: false }, frame.width, frame.height);
  const freeX = Math.max(0, frame.width - w - 2 * m);
  const freeY = Math.max(0, frame.height - h - 2 * m);
  const along = (at: number, free: number) => (free > 0 ? Math.min(1, Math.max(0, (at - m) / free)) : 0);
  return { x: along(rect.x, freeX), y: along(rect.y, freeY), size };
}

/** A box near a corner or edge sits on it. */
export function snapLayout<T extends Pick<CamLayout, "x" | "y">>(layout: T): T {
  const snap = (v: number) => (v < SNAP ? 0 : v > 1 - SNAP ? 1 : v);
  return { ...layout, x: snap(layout.x), y: snap(layout.y) };
}
