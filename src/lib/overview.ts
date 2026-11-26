import { formatBufferSeconds } from "./format.ts";
import { nearestStep } from "./videoPlan.ts";
import type { Clip } from "./types.ts";


/**
 * `armed`: off, but nothing is missed — it starts by itself with the next
 * game. Only `off` is a warning.
 */
export type BufferState = "protecting" | "waiting" | "recording" | "armed" | "off";

export interface BufferStatus {
  state: BufferState;
  title: string;
  /** Under the title; empty when there is nothing to add. */
  sub: string;
  /** How full the buffer is, 0 to 1. */
  fill: number;
}

/** What the status card says: is anything being saved right now? */
export function bufferStatus(input: {
  bufferActive: boolean;
  game: string | null;
  bufferedSeconds: number;
  bufferSeconds: number;
  /** A recording is running: the buffer being off is then not "nothing saved". */
  recording?: boolean;
  /**
   * Automatic start, only in game: the buffer is meant to be off until a game
   * is in the foreground, and starts by itself then. With a game there and the
   * buffer still off, it was stopped by hand — that is "off" again.
   */
  startsWithGame?: boolean;
}): BufferStatus {
  const { bufferActive, game, bufferedSeconds, bufferSeconds, recording, startsWithGame } = input;
  if (!bufferActive && recording) {
    return { state: "recording", title: "Buffer is off · recording", sub: "", fill: 0 };
  }
  if (!bufferActive && startsWithGame && !game) {
    return { state: "armed", title: "Starts with your next game", sub: "", fill: 0 };
  }
  if (!bufferActive) {
    return { state: "off", title: "Buffer is off — nothing is being saved", sub: "", fill: 0 };
  }
  const shown = Math.max(0, Math.round(bufferedSeconds));
  return {
    state: game ? "protecting" : "waiting",
    title: game ? `Buffering ${game}` : "Ready · waiting for a game",
    sub: `${formatBufferSeconds(shown)} of ${formatBufferSeconds(bufferSeconds)} ready to save`,
    fill: bufferSeconds > 0 ? Math.min(1, Math.max(0, bufferedSeconds / bufferSeconds)) : 0,
  };
}

/** The newest `count` items of every kind — clips, recordings, screenshots. */
export function latestItems(clips: Clip[], count = 3): Clip[] {
  return [...clips].sort((a, b) => b.createdAt - a.createdAt).slice(0, count);
}

export const JUST_NOW_MS = 60_000;

/** Younger than a minute: the "Just now" badge. */
export function isJustNow(createdAt: number, now: number): boolean {
  return now - createdAt < JUST_NOW_MS;
}

/** How long until the badge is due to go, or null when it is not showing. */
export function justNowLeft(createdAt: number, now: number): number | null {
  return isJustNow(createdAt, now) ? Math.max(0, createdAt + JUST_NOW_MS - now) : null;
}

/** The empty state's last step: the save key when bound, else the plain words. */
export function saveStep(keyLabel: string | null): string {
  return keyLabel ? `Press ${keyLabel}` : "Save a clip";
}

/** The muted line beside the action row: what a clip will be made of, as the Video page words it. */
export function videoSummary(
  rec: { height: number; fps: number; quality: number },
  encoder: string,
  fixedBitrate: boolean,
): string {
  return `${rec.height}p${rec.fps} · ${nearestStep(rec.quality).label} · ${encoder}${
    fixedBitrate ? " · fixed bitrate" : ""
  }`;
}

/**
 * The status card's warning when clips would come out silent: no audio
 * source, or none of them switched on. `null` when something is recorded.
 */
export function silentWarning(sources: ReadonlyArray<{ enabled: boolean }>): string | null {
  if (sources.length === 0) return "No audio source — clips are silent";
  if (!sources.some((source) => source.enabled)) return "All audio sources are off — clips are silent";
  return null;
}

/** The empty state's steps: with the buffer starting by itself, a game is the first. */
export function firstSteps(startsWithGame: boolean, saveKey: string | null): string[] {
  return startsWithGame ? ["Start a game", saveStep(saveKey)] : ["Start the buffer", "Play", saveStep(saveKey)];
}

export function audioSummary(enabled: number): string {
  return `${enabled} audio source${enabled === 1 ? "" : "s"}`;
}
