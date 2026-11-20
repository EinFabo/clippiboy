import type { OverlayCorner, PadAction, PadBindings, PadProfiles, PadType } from "./types";
import { PAD_ACTIONS, PROFILE_NAMES, triggerOf } from "./hotkeyText.ts";

/** The corner as words, as in the position pad's labels. */
export const CORNER_WORDS: Record<OverlayCorner, string> = {
  topLeft: "top left",
  topRight: "top right",
  bottomLeft: "bottom left",
  bottomRight: "bottom right",
};

/** "3.5 s", "2 s". */
export function secondsText(ms: number): string {
  return `${Math.round(ms / 100) / 10} s`;
}

/** The status a closed Banner group shows: screen, corner, duration. */
export function bannerSummary(screen: string, corner: OverlayCorner, durationMs: number): string {
  return `${screen} · ${CORNER_WORDS[corner]} · ${secondsText(durationMs)}`;
}

/** The status a closed Sounds group shows: device and master volume. */
export function soundsSummary(deviceName: string | null, volume: number): string {
  return `${deviceName ?? "Windows default"} · ${Math.round(volume * 100)} %`;
}

const PAD_TYPE_NAMES: Record<PadType, string> = {
  xbox: "Xbox",
  playstation: "PlayStation",
  nintendo: "Nintendo",
  other: "Other",
};

/**
 * The status a closed Controller group shows: the profile in force (a pad
 * type's own, else All) and how many of the actions have a button.
 */
export function controllerSummary(
  profiles: PadProfiles,
  seen: PadType[],
  connected: PadType[],
  lastUsed: PadType | null,
): string {
  if (seen.length === 0 && connected.length === 0) return "No controller seen";
  const type = connected[0] ?? lastUsed ?? seen[seen.length - 1] ?? null;
  const own = type === "xbox" || type === "playstation" || type === "nintendo" ? profiles[type] : null;
  const set: PadBindings = own ?? profiles.all;
  const bound = PAD_ACTIONS.filter((action: PadAction) => triggerOf(set[action]) !== "").length;
  const name = own && type ? PROFILE_NAMES[type as "xbox" | "playstation" | "nintendo"] : type ? PAD_TYPE_NAMES[type] : PROFILE_NAMES.all;
  return `${name} · ${bound} ${bound === 1 ? "button" : "buttons"} bound`;
}

/** The status a closed Stream Deck group shows. */
export function streamDeckSummary(enabled: boolean, port: number): string {
  return enabled ? `Allowed · port ${port}` : "Off";
}
