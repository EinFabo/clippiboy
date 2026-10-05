import picture from "../../src-tauri/assets/watermark.png";

/**
 * The watermark as the image editor previews it.
 *
 * A copy of `watermark::rect_kept` in the core — the editor has to show the
 * mark where the core will put it, live while a crop is dragged. Change one,
 * change both.
 */
export const WATERMARK_SRC = picture;
export const WATERMARK_OPACITY = 0.65;

const PICTURE_WIDTH = 535;
const PICTURE_HEIGHT = 152;
const HEIGHT_SHARE = 0.07;
const MARGIN_SHARE = 0.015;
const MIN_HEIGHT = 14;

export interface MarkRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Where the mark lands in an edited picture of this size. */
export function markRect(frameWidth: number, frameHeight: number): MarkRect | null {
  frameWidth = Math.round(frameWidth);
  frameHeight = Math.round(frameHeight);
  const margin = Math.round(frameHeight * MARGIN_SHARE);
  const widthFor = (height: number) => Math.floor((height * PICTURE_WIDTH) / PICTURE_HEIGHT);
  const heightFor = (width: number) => Math.floor((width * PICTURE_HEIGHT) / PICTURE_WIDTH);

  let height = Math.round(frameHeight * HEIGHT_SHARE);
  let width = widthFor(height);
  // The ordinary rule, as a fresh screenshot gets it.
  if (height >= MIN_HEIGHT && width + 2 * margin <= Math.floor(frameWidth / 2)) {
    return { x: margin, y: frameHeight - margin - height, width, height };
  }
  // Otherwise kept at any price — smallest legible, shrunk to fit.
  height = Math.max(height, MIN_HEIGHT);
  width = widthFor(height);
  const roomWidth = Math.max(frameWidth - 2 * margin, 0);
  const roomHeight = Math.max(frameHeight - 2 * margin, 0);
  if (width > roomWidth) {
    width = roomWidth;
    height = heightFor(width);
  }
  if (height > roomHeight) {
    height = roomHeight;
    width = widthFor(height);
  }
  if (width === 0 || height === 0) return null;
  return { x: margin, y: frameHeight - margin - height, width, height };
}
