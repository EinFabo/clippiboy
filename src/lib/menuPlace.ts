/** An anchor: a button's rectangle, or a point (left = right, top = bottom). */
export interface Anchor {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface PlaceInput {
  anchor: Anchor;
  /** The menu's natural size, measured without any height limit. */
  size: { width: number; height: number };
  viewport: { width: number; height: number };
  /** Distance kept to every window edge. */
  margin?: number;
  /** Space between the anchor and the menu. */
  gap?: number;
  /** The menu's right edge sits on the anchor's right edge, not its left. */
  alignRight?: boolean;
  /**
   * Whether a menu that fits the window but neither side of its anchor may
   * slide over the anchor to show whole (the default). Off for a list that
   * belongs under its button: that one is capped and scrolls instead.
   */
  slide?: boolean;
}

export interface Placement {
  x: number;
  y: number;
  /** The most the menu may be tall: what is free on the chosen side, or the
   *  whole window for a menu that slides over its anchor. */
  maxHeight: number;
  placement: "below" | "above";
}

/**
 * Where a menu goes so that it stays inside the window: below the anchor when
 * it fits, else above when it fits there, else on the larger side — shifted
 * until it shows whole when the window can hold it, with its height capped
 * (the list inside scrolls) only when it cannot.
 */
export function placeMenu({
  anchor,
  size,
  viewport,
  margin = 8,
  gap = 0,
  alignRight = false,
  slide = true,
}: PlaceInput): Placement {
  const below = viewport.height - anchor.bottom - gap - margin;
  const above = anchor.top - gap - margin;
  const placement: Placement["placement"] =
    size.height <= below ? "below" : size.height <= above || above > below ? "above" : "below";
  const side = placement === "below" ? below : above;
  // Never more than the window offers between the margins. A menu the window
  // can hold is never cut, though: it slides over its anchor instead.
  const full = viewport.height - 2 * margin;
  const maxHeight = Math.max(0, slide && size.height > side && size.height <= full ? full : Math.min(side, full));
  const height = Math.min(size.height, maxHeight);

  const y = placement === "below" ? anchor.bottom + gap : anchor.top - gap - height;
  const x = alignRight ? anchor.right - size.width : anchor.left;
  const clamp = (value: number, extent: number, limit: number) =>
    Math.max(margin, Math.min(value, limit - extent - margin));
  return {
    x: clamp(x, size.width, viewport.width),
    y: clamp(y, height, viewport.height),
    maxHeight,
    placement,
  };
}
