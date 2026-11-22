import { formatBufferSeconds } from "./format.ts";

/** The buffer lengths offered as steps, in seconds. Anything else is "Custom". */
export const BUFFER_PRESETS = [30, 60, 120, 300] as const;

/** The step a buffer length sits on, or `null` when it is none of them. */
export function bufferPreset(seconds: number): number | null {
  return BUFFER_PRESETS.find((p) => p === seconds) ?? null;
}

/** "30 s", "1 min", "2 min", "5 min" — short enough for a segment. */
export function bufferPresetLabel(seconds: number): string {
  return seconds < 60 ? `${seconds} s` : `${seconds / 60} min`;
}

/**
 * What the packet ring is allowed to hold, in megabytes.
 *
 * Mirrors `config::effective_memory_bytes`: `0` derives it from bitrate and
 * buffer length with half again on top for the peaks a variable bitrate makes.
 */
export function memoryBudgetMb(
  rec: { bitrateKbps: number },
  buffer: { seconds: number; memoryMb: number },
): number {
  if (buffer.memoryMb > 0) return buffer.memoryMb;
  return Math.max(64, memoryNeededMb(rec.bitrateKbps, buffer.seconds));
}

/** What a buffer of this length takes at this bitrate, peaks included. */
export function memoryNeededMb(bitrateKbps: number, seconds: number): number {
  return Math.round((((bitrateKbps / 8 / 1024) * seconds) * 3) / 2);
}

/**
 * How long a saved clip is. Mirrors `config::effective_clip_seconds`: `0` means
 * the whole buffer, and nothing can be longer than what is buffered.
 */
export function clipLength(buffer: { seconds: number; clipSeconds: number }): number {
  if (buffer.clipSeconds === 0) return buffer.seconds;
  return Math.min(buffer.clipSeconds, buffer.seconds);
}

/**
 * What the finished file costs. The picture is copied straight out of the
 * buffer, so its bitrate is exactly the recording bitrate — the only thing on
 * top is the one AAC track carrying the mix.
 */
export function clipMegabytes(bitrateKbps: number, seconds: number): number {
  return Math.round(((bitrateKbps + 192) / 8 / 1024) * seconds);
}

/** "about N MB per minute" — the same maths as a clip, for 60 s. */
export function megabytesPerMinute(bitrateKbps: number): number {
  return clipMegabytes(bitrateKbps, 60);
}

/** The memory limit's header line: reassurance, or how short the buffer falls. */
export function memoryLimitSummary(
  limitMb: number,
  bitrateKbps: number,
  seconds: number,
): { text: string; warn: boolean } {
  const needed = memoryNeededMb(bitrateKbps, seconds);
  if (limitMb >= needed) {
    return { text: `${limitMb} MB · enough for the full ${formatBufferSeconds(seconds)}`, warn: false };
  }
  // Seconds the limit holds at this bitrate, rounded down to whole seconds.
  const holds = Math.max(1, Math.floor((seconds * limitMb) / needed));
  return { text: `${limitMb} MB · only about ${formatBufferSeconds(holds)} at this quality`, warn: true };
}
