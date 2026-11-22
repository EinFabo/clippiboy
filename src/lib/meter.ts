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

/**
 * A level this loud is about to clip (≈ −1 dBFS). The core sends levels after
 * the source's gain and does not cap them at 1.0, so a source turned up too far
 * reaches it too.
 */
export const HOT_LEVEL = 0.89;
/** How long the warning stays after the last level that hot. */
export const HOT_HOLD_MS = 1200;

/**
 * Until when the knob shows that the source is about to clip: every hot level
 * starts the hold anew, so a loud passage does not make it flicker.
 */
export function hotUntil(until: number, level: number, now: number): number {
  return level >= HOT_LEVEL ? now + HOT_HOLD_MS : until;
}

/** A peak shown as a number: held a second, so it can be read. */
export interface HeldPeak {
  level: number;
  at: number;
}

export const PEAK_HOLD_MS = 1000;

/** The peak to show after a new `level`: a higher one at once, a lower one only
 *  once the held peak is a second old. */
export function holdPeak(held: HeldPeak, level: number, now: number): HeldPeak {
  return level >= held.level || now - held.at >= PEAK_HOLD_MS
    ? { level, at: now }
    : held;
}

/** A peak as the dB column shows it: whole dB, "Silent" below the meter's −60. */
export function peakDbText(level: number): string {
  if (!(level > 0.001)) return "Silent";
  const db = Math.round(20 * Math.log10(level));
  // Math.round(-0.4) is -0, which would print as "-0".
  return `${db > 0 ? "+" : db < 0 ? "-" : ""}${Math.abs(db)} dB`;
}
