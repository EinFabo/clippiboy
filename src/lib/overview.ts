import { formatBufferSeconds } from "./format.ts";
import { nearestStep } from "./videoPlan.ts";
import type { Clip } from "./types.ts";


export type BufferState = "protecting" | "waiting" | "recording" | "off";

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
}): BufferStatus {
  const { bufferActive, game, bufferedSeconds, bufferSeconds, recording } = input;
  if (!bufferActive && recording) {
    return { state: "recording", title: "Buffer is off · recording", sub: "", fill: 0 };
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

export function audioSummary(enabled: number): string {
  return `${enabled} audio source${enabled === 1 ? "" : "s"}`;
}
