import type { FriendsConfig, OverlayConfig, SoundKind } from "./types";

/** Where an event's banner switch lives: the banner's own settings or the friends'. */
export type BannerField =
  | { scope: "overlay"; key: keyof Pick<OverlayConfig, "onClipSaved" | "onScreenshot" | "onRecording" | "onBufferToggle" | "onError"> }
  | { scope: "friends"; key: keyof Pick<FriendsConfig, "notifyRequests" | "notifyOnline" | "notifyGames" | "notifyClips"> };

export interface NotifyEvent {
  label: string;
  banner: BannerField | null;
  sound: SoundKind | null;
  /** The core plays this sound only inside the banner's alert, so with the
      banner off the sound never comes. */
  soundNeedsBanner?: boolean;
  /** A switch narrowing the banner to favorites, shown beside it. */
  favoritesOnly?: keyof Pick<FriendsConfig, "notifyGamesFavoritesOnly">;
}

/** One row of Settings → Notifications each, in the order they come up while playing. */
export const NOTIFY_EVENTS: NotifyEvent[] = [
  { label: "Clip saved", banner: { scope: "overlay", key: "onClipSaved" }, sound: "clipSaved" },
  { label: "Screenshot taken", banner: { scope: "overlay", key: "onScreenshot" }, sound: "screenshot" },
  { label: "Recording started", banner: { scope: "overlay", key: "onRecording" }, sound: "recordingStarted" },
  { label: "Recording saved", banner: { scope: "overlay", key: "onRecording" }, sound: "recordingSaved" },
  { label: "Buffer on", banner: { scope: "overlay", key: "onBufferToggle" }, sound: "bufferOn" },
  { label: "Buffer off", banner: { scope: "overlay", key: "onBufferToggle" }, sound: "bufferOff" },
  { label: "A friend comes online", banner: { scope: "friends", key: "notifyOnline" }, sound: "friendOnline", soundNeedsBanner: true },
  {
    label: "A friend starts a game",
    banner: { scope: "friends", key: "notifyGames" },
    sound: null,
    favoritesOnly: "notifyGamesFavoritesOnly",
  },
  { label: "Friend requests", banner: { scope: "friends", key: "notifyRequests" }, sound: null },
  { label: "Clips with friends", banner: { scope: "friends", key: "notifyClips" }, sound: "clipReceived" },
  { label: "Errors", banner: { scope: "overlay", key: "onError" }, sound: "error" },
];
