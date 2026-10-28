// What an export will do, worked out the same way `export::recipe` does it.
// Pure, so it can be tested. It runs on every keystroke in the size field, so
// it is duplicated here rather than asked of the core; keep it in step with
// `export.rs` and `encode::bitrate_for`.

const HEIGHTS = [1080, 720, 480];
const AUDIO_KBPS = 128;
const THIN_AUDIO_KBPS = 64;
const VIDEO_FLOOR_KBPS = 200;
const CONTAINER_OVERHEAD = 3;
const STARVED = 2;
const BITS_PER_PIXEL = Math.fround(0.15);

/** Mirrors `encode::bitrate_for`, f32 arithmetic included. */
export function bitrateFor(width: number, height: number, fps: number): number {
  const pixelsPerSecond = width * height * Math.max(1, fps);
  const kbps = Math.floor(Math.fround(Math.fround(pixelsPerSecond) * BITS_PER_PIXEL) / 1000);
  const rounded = Math.floor((kbps + 500) / 1000) * 1000;
  return Math.min(100_000, Math.max(2_000, rounded));
}

function widthFor(clip: { width: number; height: number }, height: number): number {
  if (clip.height === 0) return clip.width;
  const width = Math.floor((clip.width * height) / clip.height);
  return Math.max(2, width) & ~1;
}

function videoBudget(targetBytes: number, durationMs: number, audioKbps: number): number {
  const usableBits = Math.floor((Math.floor(targetBytes) * 8 * (100 - CONTAINER_OVERHEAD)) / 100);
  const videoBits = Math.max(0, usableBits - audioKbps * durationMs);
  return Math.floor(videoBits / Math.max(1000, durationMs));
}

/**
 * Mirrors `export::video_budget` and `export::recipe`. `seconds` is for
 * display; `tooLong` says the target cannot be met, and `expectedMb` is then
 * about what the copy will come to instead.
 */
export function planExport(
  clip: { durationMs: number; width: number; height: number },
  targetBytes: number,
): {
  videoKbps: number;
  audioKbps: number;
  width: number;
  height: number;
  seconds: number;
  tooLong: boolean;
  expectedMb: number;
} {
  let audioKbps = AUDIO_KBPS;
  let budget = videoBudget(targetBytes, clip.durationMs, audioKbps);
  if (budget < VIDEO_FLOOR_KBPS) {
    audioKbps = THIN_AUDIO_KBPS;
    budget = videoBudget(targetBytes, clip.durationMs, audioKbps);
  }

  let height = clip.height;
  for (const step of [clip.height, ...HEIGHTS.filter((h) => h < clip.height)]) {
    height = step;
    const wants = bitrateFor(widthFor(clip, step), step, 30);
    if (budget >= Math.floor(wants / STARVED)) break;
  }
  const videoKbps = Math.max(VIDEO_FLOOR_KBPS, budget);
  const expectedBytes = (((videoKbps + audioKbps) * clip.durationMs) / 8) * (100 / (100 - CONTAINER_OVERHEAD));
  return {
    videoKbps,
    audioKbps,
    width: widthFor(clip, height),
    height,
    seconds: Math.round(clip.durationMs / 100) / 10,
    tooLong: budget < VIDEO_FLOOR_KBPS,
    expectedMb: roundMb(expectedBytes / (1024 * 1024)),
  };
}

/** A size in MB to one decimal, as the core's `export::megabytes` shows it. */
export function roundMb(mb: number): number {
  return Math.round(mb * 10) / 10;
}
