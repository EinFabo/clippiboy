import {
	action,
	type DidReceiveSettingsEvent,
	type KeyAction,
	type KeyDownEvent,
	type WillAppearEvent,
	type WillDisappearEvent,
} from "@elgato/streamdeck";

import { clippiboy, type Status } from "../client.js";
import { bufferLabel, ClippiBoyKey } from "../keys.js";

/** Set per key in the property inspector. */
export type SaveClipSettings = {
	/** How much of the buffer this key saves. Missing or 0: the clip length set in ClippiBoy. */
	seconds?: number;
};

/**
 * Writes the last seconds out of the replay buffer — the same route the hotkey
 * takes, only with the buffer level on the key.
 *
 * Each key can carry a length of its own: one on 1:00, one on 2:30, both out of
 * the same three-minute buffer. ClippiBoy caps it at the buffer.
 */
@action({ UUID: "com.einfabo.clippiboy.save" })
export class SaveClip extends ClippiBoyKey {
	/** The length per visible key — `draw` only gets the key, not its settings. */
	readonly #seconds = new Map<string, number>();

	override onWillAppear(ev: WillAppearEvent<SaveClipSettings>): void {
		this.#seconds.set(ev.action.id, ev.payload.settings.seconds ?? 0);
		super.onWillAppear(ev);
	}

	override onWillDisappear(ev: WillDisappearEvent): void {
		this.#seconds.delete(ev.action.id);
		super.onWillDisappear(ev);
	}

	override async onDidReceiveSettings(ev: DidReceiveSettingsEvent<SaveClipSettings>): Promise<void> {
		this.#seconds.set(ev.action.id, ev.payload.settings.seconds ?? 0);
		// Straight away rather than on the next poll: the length was just picked.
		if (ev.action.isKey()) {
			await this.draw(ev.action, clippiboy.status);
		}
	}

	override async onKeyDown(ev: KeyDownEvent<SaveClipSettings>): Promise<void> {
		const seconds = ev.payload.settings.seconds ?? 0;
		await this.press(ev.action, "/v1/clip", seconds > 0 ? { seconds } : undefined);
	}

	protected override async draw(key: KeyAction, status: Status | null): Promise<void> {
		// What a press would save, not the whole buffer: with two keys on
		// different lengths, that is what tells them apart.
		const own = this.#seconds.get(key.id) ?? 0;
		const cap = own > 0 ? own : status?.clipSeconds;
		await key.setTitle(bufferLabel(status, cap));
	}
}
