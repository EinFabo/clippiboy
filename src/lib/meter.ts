/** How long one decay step of the bar takes, and how much it keeps. */
const STEP_MS = 60;
const KEEP = 0.78;

/**
 * The shown level after `dtMs` of falling: ×0.78 per 60 ms, as a smooth curve
 * over elapsed time, so the bar falls the same at any frame rate.
 */
export function decayLevel(shown: number, dtMs: number): number {
  if (shown < 0.005) return 0;
  const next = shown * Math.pow(KEEP, dtMs / STEP_MS);
  return next < 0.005 ? 0 : next;
}

/** Levels rise instantly and fall softly. */
export function riseLevel(shown: number, level: number): number {
  return level > shown ? level : shown;
}

/**
 * Linear amplitude always looks like "nothing" on a bar: normal speech sits at
 * 0.03–0.1. Hence a dB scale from -60 to 0, as a share of the bar (0–100).
 */
export function levelPct(level: number): number {
  const db = level > 0 ? 20 * Math.log10(level) : -Infinity;
  return Math.min(100, Math.max(0, ((db + 60) / 60) * 100));
}
