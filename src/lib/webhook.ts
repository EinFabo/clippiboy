import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { inTauri } from "./ipc";

/**
 * "Post to <server>": the Discord channel clips can be posted to (`webhook.rs`).
 * The core keeps the webhook itself and never hands it out — a window only
 * learns whether a channel is connected and the server's name.
 */
export interface WebhookView {
  connected: boolean;
  /** The server's name. */
  guild: string | null;
  /** The browser is open and the core waits for Discord's answer. */
  connecting: boolean;
  /** Clips waiting to be posted or going up now. */
  posting: string[];
}

export const webhookApi = {
  /** Opens the browser on Discord's server-and-channel dialog. */
  connect: () => invoke<void>("webhook_connect"),
  cancel: () => invoke<void>("webhook_cancel"),
  disconnect: () => invoke<void>("webhook_disconnect"),
  /** Queued; the result comes as a notice. */
  post: (id: string) => invoke<void>("webhook_post", { id }),
};

export const useWebhook = create<WebhookView>(() => ({
  connected: false,
  guild: null,
  connecting: false,
  posting: [],
}));

if (inTauri) {
  void listen<WebhookView>("webhook-changed", (event) => useWebhook.setState(event.payload));
  void invoke<WebhookView>("webhook_state")
    .then((view) => useWebhook.setState(view))
    .catch(() => {});
}

export { postLabel } from "./webhookText";
