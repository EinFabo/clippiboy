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

/**
 * How full a source's own signal is, before its volume (0–1): the core sends
 * levels after the gain, and a volume track draws the signal inside the part
 * that is set — the gain moves the knob, so it must not move the bar as well.
 */
export function preGain(level: number, gainDb: number): number {
  const pre = level / 10 ** (gainDb / 20);
  return pre > 0 ? Math.min(1, pre) : 0;
}

/**
 * How long a changed volume may still be missing from the levels: the write
 * goes to the core, the core applies it, and packets measured before that are
 * still on their way.
 */
export const GAIN_SETTLE_MS = 500;

/** The higher of the gains that may still be in the levels, and until when. */
export interface GainHold {
  db: number;
  until: number;
}

/**
 * The hold after the volume changed away from `fromDb`: that gain may still be
 * in the next packets. A change during a hold keeps the higher of the two.
 */
export function holdGain(hold: GainHold | null, fromDb: number, now: number): GainHold {
  const db = hold && now < hold.until ? Math.max(hold.db, fromDb) : fromDb;
  return { db, until: now + GAIN_SETTLE_MS };
}

/**
 * The gain to take out of a level. Right after a change the packets may carry
 * the old gain or the new one; the higher of the two can only make the bar
 * read low for a moment — a big drop taken out of packets still measured at
 * the old volume filled the whole bar.
 */
export function meterGain(gainDb: number, hold: GainHold | null, now: number): number {
  return hold && now < hold.until ? Math.max(gainDb, hold.db) : gainDb;
}

/**
 * Browser mode's stand-in for the core's level of a source: `raw` (the
 * signal, 0–1) after the source's volume, nothing while it is off, muted or
 * left out by another source's Solo — what the core reports.
 */
export function simulatedLevel(
  source: { enabled: boolean; muted: boolean; solo: boolean; gainDb: number },
  anySolo: boolean,
  raw: number,
): number {
  if (!source.enabled || source.muted || (anySolo && !source.solo)) return 0;
  return raw * 10 ** (source.gainDb / 20);
}
