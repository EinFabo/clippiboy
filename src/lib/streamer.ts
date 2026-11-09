import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { inTauri } from "./ipc";
import type { EngineStatus } from "./types";

/**
 * Whether streamer mode is in effect right now (`streamer.rs`). The core says
 * so in `streamer-mode` the moment it changes and in every `engine-status`;
 * every window — the console too — mirrors it here to hide names, avatars,
 * friend codes and links while it is on.
 */
export const useStreamer = create<{ active: boolean }>(() => ({ active: false }));

function take(active: boolean) {
  if (useStreamer.getState().active !== active) useStreamer.setState({ active });
}

if (inTauri) {
  void listen<boolean>("streamer-mode", (event) => take(event.payload));
  void listen<EngineStatus>("engine-status", (event) => take(!!event.payload.streamer));
  void invoke<EngineStatus>("engine_status")
    .then((status) => take(!!status.streamer))
    .catch(() => {});
}

export { HIDDEN_NAME, shownCode, shownName } from "./streamerText";
