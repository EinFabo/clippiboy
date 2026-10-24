import type { CaptureTarget } from "./types.ts";

/** The usual frame rates, as long as the screen does not reveal its own. */
export const FPS = [30, 60, 120];

/**
 * Which frame rates the source really supports.
 *
 * Capturing more frames than the screen puts out does not make any motion
 * smoother — it only produces duplicate frames that cost space. Hence the usual
 * steps up to the refresh rate plus that rate itself: a 165 Hz monitor should
 * offer 165 too. That is how the core computes it in `capture::fps_choices`.
 */
export function fpsChoices(source: CaptureTarget | null): number[] {
  const refresh = source?.refreshHz ?? null;
  if (!refresh || refresh < 20) return FPS;
  return [...FPS.filter((fps) => fps < refresh), refresh];
}

/** Snap the configured frame rate onto an offered step — downwards, because
    capturing more than was set would be a surprise. */
export function fitFps(fps: number, choices: number[]): number {
  if (choices.includes(fps)) return fps;
  return [...choices].reverse().find((step) => step <= fps) ?? choices[0];
}

/**
 * Capture size for a target height. The width comes from the source's aspect
 * ratio instead of an assumed 16:9, and both values stay even — exactly how the
 * core computes it in `capture::fit_to_target`.
 */
export function fit(height: number, source: CaptureTarget | null) {
  const ratio =
    source && source.width > 0 && source.height > 0
      ? source.width / source.height
      : 16 / 9;
  const h = Math.max(2, Math.round(height)) & ~1;
  const w = Math.max(2, Math.round(h * ratio)) & ~1;
  return { width: w, height: h };
}

/**
 * What the Video page saves when the source changes, or `null` for "leave the
 * config alone". A source that only stands in for a missing choice (unplugged
 * screen, closed game window, a target list that is still from app start) must
 * not rewrite the saved size and rate: they come back with the real source.
 */
export function autoFit(
  rec: { width: number; height: number; fps: number },
  source: CaptureTarget | null,
  missing: boolean,
): { width: number; height: number; fps: number } | null {
  if (!source || missing) return null;
  const next = fit(Math.min(rec.height, source.height), source);
  const fps = fitFps(rec.fps, fpsChoices(source));
  const sized = next.width !== rec.width || next.height !== rec.height;
  return sized || fps !== rec.fps ? { ...next, fps } : null;
}
