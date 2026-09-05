import { action, type KeyAction, type KeyDownEvent } from "@elgato/streamdeck";

import type { Status } from "../client.js";
import { bufferLabel, ClippiBoyKey } from "../keys.js";

/**
 * Writes the last seconds out of the replay buffer — the same route the hotkey
 * takes, only with the buffer level on the key.
 */
@action({ UUID: "com.einfabo.clippiboy.save" })
export class SaveClip extends ClippiBoyKey {
	override async onKeyDown(ev: KeyDownEvent): Promise<void> {
		await this.press(ev.action, "/v1/clip");
	}

	protected override async draw(key: KeyAction, status: Status | null): Promise<void> {
		await key.setTitle(bufferLabel(status));
	}
}
