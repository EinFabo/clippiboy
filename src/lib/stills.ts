import type { CaptureThumb } from "./types";

/** How often the monitor map asks for fresh stills of the screens. */
export const STILL_EVERY_MS = 3000;

/** What the map knows at a tick. */
export interface StillTick {
  /** The page can be seen: the document is visible and the window not in the tray. */
  visible: boolean;
  /**
   * The main window has the focus. Visible is not enough: a game in front of
   * it covers the window without hiding the document.
   */
  focused: boolean;
  /** The last request has not answered yet. */
  inFlight: boolean;
  /** How many screens there are to ask about. */
  screens: number;
}

/**
 * Whether to ask for stills now. Never while the last answer is still out —
 * a slow grab skips a tick rather than piling up — never for nobody to see or
 * with another window in front, never for no screens.
 */
export function shouldFetchStills({ visible, focused, inFlight, screens }: StillTick): boolean {
  return visible && focused && !inFlight && screens > 0;
}

/** The answer as screen id → image; screens without one are left out. */
export function stillsById(thumbs: CaptureThumb[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const { id, jpeg } of thumbs) if (jpeg) out[id] = jpeg;
  return out;
}
