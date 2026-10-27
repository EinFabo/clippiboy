/**
 * The rules the core applies when it saves an edit, so the preview can follow
 * them. No imports: the tests run in plain node.
 */

/** Mirrors `edit::EDGE_TOLERANCE_MS`: a cut this close to an edge is dropped on save. */
export const EDGE_S = 0.25;

/** A cut of `EDGE_S` or less snaps to the edge, as the core treats it. */
export function snapTrim(
  start: number,
  end: number,
  duration: number,
): { start: number; end: number } {
  return {
    start: start <= EDGE_S ? 0 : start,
    end: duration > 0 && end >= duration - EDGE_S ? duration : end,
  };
}

/** Does the selection cut anything the core would really cut? */
export function isTrimmed(start: number, end: number, duration: number): boolean {
  return start > EDGE_S || end < duration - EDGE_S;
}

/**
 * The video element's volume for a clip with one audio track: muted is silent,
 * the level applies, and the element cannot boost above 1.
 */
export function singleTrackVolume(master: number, gainDb: number, muted: boolean): number {
  return muted ? 0 : Math.min(1, master * 10 ** (gainDb / 20));
}
