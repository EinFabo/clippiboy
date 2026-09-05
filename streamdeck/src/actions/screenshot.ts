import { action, type KeyAction, type KeyDownEvent } from "@elgato/streamdeck";

import type { Status } from "../client.js";
import { ClippiBoyKey } from "../keys.js";

/**
 * A screenshot needs no buffer, so this key carries no number — only whether
 * ClippiBoy is there at all.
 */
@action({ UUID: "com.einfabo.clippiboy.screenshot" })
export class Screenshot extends ClippiBoyKey {
	override async onKeyDown(ev: KeyDownEvent): Promise<void> {
		await this.press(ev.action, "/v1/screenshot");
	}

	protected override async draw(key: KeyAction, status: Status | null): Promise<void> {
		await key.setTitle(status === null ? "—" : "");
	}
}
