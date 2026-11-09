import type { DiscordLimit } from "./types.ts";

/** What Discord takes per file, by account (setting "Discord upload limit"). */
export const DISCORD_LIMITS: Record<DiscordLimit, { label: string; mb: number }> = {
  free: { label: "Free", mb: 20 },
  nitroBasic: { label: "Nitro Basic", mb: 50 },
  nitro: { label: "Nitro", mb: 500 },
};

/**
 * What a "Copy for Discord" aims at, in MB: 95 % of the limit, whole MB. An
 * encode lands near its target rather than on it, and a copy one byte over is
 * refused at the door. A config from before the setting counts as free.
 */
export function discordMb(limit: DiscordLimit | undefined): number {
  return Math.floor((DISCORD_LIMITS[limit ?? "free"] ?? DISCORD_LIMITS.free).mb * 0.95);
}
