import type { KeyAction, WillAppearEvent, WillDisappearEvent } from "@elgato/streamdeck";
import streamDeck, { SingletonAction } from "@elgato/streamdeck";

import { clippiboy, type Status } from "./client.js";

/**
 * A key that keeps itself in step with ClippiBoy.
 *
 * Every one of the three keys does the same two things: watch the status while
 * it is visible, and drop the watch when it goes away — a key on a page nobody
 * is looking at must not keep the poller alive. Only [`draw`] and what happens
 * on a press differ, so that is all the three subclasses fill in.
 */
export abstract class ClippiBoyKey extends SingletonAction {
	/** One unwatch function per visible key instance. */
	readonly #watching = new Map<string, () => void>();

	override onWillAppear(ev: WillAppearEvent): void {
		if (!ev.action.isKey()) {
			return;
		}
		const key = ev.action;
		this.#watching.get(key.id)?.();
		this.#watching.set(
			key.id,
			clippiboy.watch((status) => void this.draw(key, status)),
		);
	}

	override onWillDisappear(ev: WillDisappearEvent): void {
		this.#watching.get(ev.action.id)?.();
		this.#watching.delete(ev.action.id);
	}

	/** Bring the key in line with the status; `null` means ClippiBoy is not reachable. */
	protected abstract draw(key: KeyAction, status: Status | null): Promise<void>;

	/**
	 * Press a button and show what came of it.
	 *
	 * The checkmark is worth more here than in the app: on a Stream Deck the eyes
	 * are on the game, and a clip that quietly failed would only turn up hours
	 * later when it is not in the folder.
	 */
	protected async press(key: KeyAction, path: string, body?: unknown): Promise<void> {
		const answer = await clippiboy.post(path, body);
		if (answer === null) {
			streamDeck.logger.info(`${path}: ClippiBoy is not running`);
			await key.showAlert();
			return;
		}
		if (!answer.ok) {
			streamDeck.logger.warn(`${path} failed: ${answer.error ?? "no reason given"}`);
			await key.showAlert();
			return;
		}
		await key.showOk();
	}
}

/**
 * How much is in the buffer, in as few characters as a key can carry.
 *
 * A dash for "no ClippiBoy" rather than an alarm icon: the app is not meant to
 * run around the clock, and a key that cries for help all evening teaches you
 * to ignore it.
 */
export function bufferLabel(status: Status | null): string {
	if (status === null) {
		return "—";
	}
	if (status.saving) {
		return "…";
	}
	if (!status.bufferActive) {
		return "off";
	}
	const seconds = Math.round(status.bufferedSeconds);
	if (seconds < 60) {
		return `${seconds}s`;
	}
	return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}
