import type { Status } from "./client.js";

/**
 * How much is in the buffer, in as few characters as a key can carry — or, with
 * `cap`, how much of it a press would save.
 *
 * A dash for "no ClippiBoy" rather than an alarm icon: the app is not meant to
 * run around the clock, and a key that cries for help all evening teaches you
 * to ignore it.
 */
export function bufferLabel(status: Status | null, cap?: number): string {
	if (status === null) {
		return "—";
	}
	if (status.saving) {
		return "…";
	}
	if (!status.bufferActive) {
		return "off";
	}
	return durationLabel(Math.min(Math.round(status.bufferedSeconds), cap ?? Infinity));
}

/** `45s`, `2:30` — as short as a key can carry it. */
export function durationLabel(seconds: number): string {
	if (seconds < 60) {
		return `${seconds}s`;
	}
	return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

/** How long the recording runs — `12:34` or `1:02:03` — or nothing to say. */
export function recordingLabel(status: Status | null): string {
	if (status === null) {
		return "—";
	}
	if (status.finalizing) {
		// Stopped, but the file is still being written: not "off" yet.
		return "…";
	}
	if (!status.recording) {
		return "";
	}
	const total = Math.floor(status.recordingSeconds ?? 0);
	const hours = Math.floor(total / 3600);
	const minutes = Math.floor(total / 60) % 60;
	const seconds = String(total % 60).padStart(2, "0");
	return hours > 0
		? `${hours}:${String(minutes).padStart(2, "0")}:${seconds}`
		: `${Math.floor(total / 60)}:${seconds}`;
}
