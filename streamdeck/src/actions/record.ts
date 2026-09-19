import { action, type KeyAction, type KeyDownEvent } from "@elgato/streamdeck";

import type { Status } from "../client.js";
import { ClippiBoyKey, recordingLabel } from "../keys.js";

/**
 * A recording on and off.
 *
 * Like the buffer key, the state comes from the poll: a recording is also
 * started by hotkey, from the tray and in the app. The press answers only once
 * a stopped recording is written, so the check mark means the file is there.
 */
@action({ UUID: "com.einfabo.clippiboy.record" })
export class Record extends ClippiBoyKey {
	override async onKeyDown(ev: KeyDownEvent): Promise<void> {
		await this.press(ev.action, "/v1/recording", { action: "toggle" });
	}

	protected override async draw(key: KeyAction, status: Status | null): Promise<void> {
		await key.setState(status?.recording ? 1 : 0);
		await key.setTitle(recordingLabel(status));
	}
}
