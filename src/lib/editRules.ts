/**
 * The rules the core applies when it saves an edit, so the preview can follow
 * them. No imports: the tests run in plain node.
 */

/** Mirrors `edit::EDGE_TOLERANCE_MS`: a cut this close to an edge is dropped on save. */
export const EDGE_S = 0.25;

/** Mirrors `stems::SILENT_DB`: at or below this a track is dropped on save. */
export const SILENT_DB = -59;

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
 * Does saving this selection need a yes first? Only a cut on a recording: it
 * keeps no untouched copy (`edit::archives_the_clip`), so its trim is final. An
 * older recording whose copy is still beside it can be undone like a clip.
 */
export function asksBeforeTrim(
  clip: { recording: boolean; originalAvailable: boolean },
  start: number,
  end: number,
  duration: number,
): boolean {
  return clip.recording && !clip.originalAvailable && isTrimmed(start, end, duration);
}

/**
 * The video element's volume for a clip with one audio track: muted is silent,
 * the level applies, and the element cannot boost above 1.
 */
export function singleTrackVolume(master: number, gainDb: number, muted: boolean): number {
  return muted || gainDb <= SILENT_DB ? 0 : Math.min(1, master * 10 ** (gainDb / 20));
}

/**
 * Do two selections save the same cut? Both are snapped by the edge rule first
 * (a cut under a quarter second is no cut), then compared with a small slack:
 * the length the video element reports varies by fractions between two loads.
 */
export function sameCut(
  a: { start: number; end: number },
  b: { start: number; end: number },
  duration: number,
): boolean {
  const x = snapTrim(a.start, a.end, duration);
  const y = snapTrim(b.start, b.end, duration);
  return Math.abs(x.start - y.start) <= 0.05 && Math.abs(x.end - y.end) <= 0.05;
}
