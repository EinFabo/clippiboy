import { action, type KeyAction, type KeyDownEvent } from "@elgato/streamdeck";

import type { Status } from "../client.js";
import { bufferLabel, ClippiBoyKey } from "../keys.js";

/**
 * The replay buffer on and off.
 *
 * The state comes from the poll, not from the press: the buffer is also switched
 * by hotkey, by the tray and by ClippiBoy itself when a game starts, and a key
 * that only remembered its own presses would show the opposite of the truth
 * within a minute.
 */
@action({ UUID: "com.einfabo.clippiboy.buffer" })
export class ToggleBuffer extends ClippiBoyKey {
	override async onKeyDown(ev: KeyDownEvent): Promise<void> {
		await this.press(ev.action, "/v1/buffer", { action: "toggle" });
	}

	protected override async draw(key: KeyAction, status: Status | null): Promise<void> {
		await key.setState(status?.bufferActive ? 1 : 0);
		await key.setTitle(bufferLabel(status));
	}
}
