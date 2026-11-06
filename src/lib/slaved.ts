// Elements that follow the clip's <video>: where they should be, and how to
// get them there without a visible jump.

/** Closer than this a video slave is left alone. Two slaves left at the edge
 *  of one frame each sat two frames apart: the face visibly off the game. */
const SETTLED_S = 0.01;
/** Past this a video slave seeks; below it the rate is nudged. */
const VIDEO_SEEK_S = 0.25;
/** The stems' bound since they exist. */
const AUDIO_SEEK_S = 0.12;
/** The largest rate nudge: ±3 % is not seen. */
const MAX_NUDGE = 0.03;

/** The slave's time for the master at `masterSec`; element time = clip time + lead. */
export function slaveTime(masterSec: number, leadMs: number): number {
  return Math.max(0, Math.round((masterSec + leadMs / 1000) * 1e6) / 1e6);
}

/**
 * What to do about `drift` (slave minus target, seconds). Audio seeks as the
 * stems always did; video seeks only when far off and otherwise runs a little
 * slower or faster until it is back, since a seek on H.264 decodes from the
 * last keyframe and shows as a stutter. `rate` multiplies the master's rate.
 */
export function driftAction(drift: number, kind: "audio" | "video"): { seek: boolean; rate: number } {
  const off = Math.abs(drift);
  if (kind === "audio") return { seek: off > AUDIO_SEEK_S, rate: 1 };
  if (off > VIDEO_SEEK_S) return { seek: true, rate: 1 };
  if (off <= SETTLED_S) return { seek: false, rate: 1 };
  return { seek: false, rate: 1 - Math.max(-MAX_NUDGE, Math.min(MAX_NUDGE, drift)) };
}

/** Start nudging past this, stop below `STEER_STOP_S`. */
const STEER_START_S = 0.025;
const STEER_STOP_S = 0.008;

/**
 * The rate factor for a slave whose averaged drift is `drift` seconds, with
 * hysteresis: every change of `playbackRate` makes Chromium settle anew, so a
 * rate flipped at each frame made the picture jitter by ±50 ms. `steering`
 * says whether the slave is already on its way back.
 */
export function steer(drift: number, steering: boolean): number {
  const off = Math.abs(drift);
  if (off < (steering ? STEER_STOP_S : STEER_START_S)) return 1;
  return 1 - Math.max(-MAX_NUDGE, Math.min(MAX_NUDGE, drift));
}

/** Is the clip at `clipSec` inside a stretch without a camera? The gaps are in
 *  the clean picture's milliseconds, as the composite's `enable` uses them. */
export function inGap(clipSec: number, cleanLeadMs: number, gaps: [number, number][]): boolean {
  const ms = clipSec * 1000 + cleanLeadMs;
  return gaps.some(([a, b]) => ms >= a && ms <= b);
}
