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
}

export interface Placement {
  x: number;
  y: number;
  /** The most the menu may be tall: what is free on the chosen side. */
  maxHeight: number;
  placement: "below" | "above";
}

/**
 * Where a menu goes so that it stays inside the window: below the anchor when
 * it fits, else above when it fits there, else on the larger side with its
 * height capped (the list inside scrolls).
 */
export function placeMenu({
  anchor,
  size,
  viewport,
  margin = 8,
  gap = 0,
  alignRight = false,
}: PlaceInput): Placement {
  const below = viewport.height - anchor.bottom - gap - margin;
  const above = anchor.top - gap - margin;
  const placement: Placement["placement"] =
    size.height <= below ? "below" : size.height <= above || above > below ? "above" : "below";
  // Never more than the window offers between the margins.
  const maxHeight = Math.max(0, Math.min(placement === "below" ? below : above, viewport.height - 2 * margin));
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
