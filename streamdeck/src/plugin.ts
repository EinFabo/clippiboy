import streamDeck from "@elgato/streamdeck";

import { SaveClip } from "./actions/save-clip.js";
import { Screenshot } from "./actions/screenshot.js";
import { ToggleBuffer } from "./actions/toggle-buffer.js";
import { clippiboy } from "./client.js";

// Port or token entered by hand: whatever was cached points at the old place.
streamDeck.settings.onDidReceiveGlobalSettings(() => clippiboy.forget());

// The property inspector is the same page for all three keys, so this belongs
// here and not in the actions: it asks whether ClippiBoy can be reached at all,
// and only the plugin can answer that — the page has no way to read the
// handshake file.
streamDeck.ui.onDidAppear(async () => {
	await streamDeck.ui.sendToPropertyInspector({ ...(await clippiboy.describe()) });
});

streamDeck.ui.onSendToPlugin<{ command?: string }>(async (ev) => {
	if (ev.payload?.command !== "probe") {
		return;
	}
	// Port and token were just entered by hand — whatever is cached points at the
	// old place, and the answer below has to be about the new one.
	clippiboy.forget();
	await streamDeck.ui.sendToPropertyInspector({ ...(await clippiboy.describe()) });
});

streamDeck.actions.registerAction(new SaveClip());
streamDeck.actions.registerAction(new Screenshot());
streamDeck.actions.registerAction(new ToggleBuffer());

await streamDeck.connect();
