import { discordKind, hasDiscordSource, runningDiscord } from "./discordSource.ts";
import type { AudioDevice, AudioProcess, AudioSource, SourceKind } from "./types.ts";

/** Which of Quick setup's rows are switched on. */
export interface QuickPicks {
  output: boolean;
  input: boolean;
  discord: boolean;
}

/** What Quick setup starts with — and what "Skip setup" takes unseen. */
export const QUICK_PICKS: QuickPicks = { output: true, input: true, discord: true };

/** What Quick setup is read from: the store's lists as they stand. */
export interface QuickFacts {
  devices: AudioDevice[];
  sources: AudioSource[];
  processes: AudioProcess[];
  /** Per-app capture (Windows 11); `null` until the core has answered. */
  appCapture: boolean | null;
}

/**
 * The three rows Quick setup can offer, each with whether the list holds it
 * already. A row is missing when there is nothing behind it: no default
 * output, no default input, no Discord running (or no per-app capture).
 */
export interface QuickOffer {
  output?: { device: AudioDevice; added: boolean };
  input?: { device: AudioDevice; added: boolean };
  /** `process` is unset when Discord is on its own track already but not running. */
  discord?: { process?: AudioProcess; added: boolean };
}

export function quickOffer({ devices, sources, processes, appCapture }: QuickFacts): QuickOffer {
  const has = (type: "outputDevice" | "inputDevice", id: string) =>
    sources.some((s) => s.kind.type === type && "deviceId" in s.kind && s.kind.deviceId === id);
  const output = devices.find((d) => d.kind === "output" && d.isDefault);
  const input = devices.find((d) => d.kind === "input" && d.isDefault);
  const offer: QuickOffer = {};
  if (output) offer.output = { device: output, added: has("outputDevice", output.id) };
  if (input) offer.input = { device: input, added: has("inputDevice", input.id) };
  // Discord has its own track only through per-app capture.
  if (appCapture !== false) {
    const process = runningDiscord(processes);
    const added = hasDiscordSource(sources);
    if (process || added) offer.discord = { process, added };
  }
  return offer;
}

/**
 * The sources the switches add: the ones on and not in the list yet, named
 * after the device as picking it by hand would, Discord as "Discord".
 */
export function quickAdds(facts: QuickFacts, picks: QuickPicks): Array<{ label: string; kind: SourceKind }> {
  const offer = quickOffer(facts);
  const adds: Array<{ label: string; kind: SourceKind }> = [];
  if (picks.output && offer.output && !offer.output.added) {
    adds.push({ label: offer.output.device.name, kind: { type: "outputDevice", deviceId: offer.output.device.id } });
  }
  if (picks.input && offer.input && !offer.input.added) {
    adds.push({ label: offer.input.device.name, kind: { type: "inputDevice", deviceId: offer.input.device.id } });
  }
  if (picks.discord && offer.discord?.process && !offer.discord.added) {
    adds.push({ label: "Discord", kind: discordKind(offer.discord.process) });
  }
  return adds;
}

/**
 * Does "Skip setup" add Quick setup's sources? Only to an empty list, and
 * only while the audio step has not been answered — a "Later" there, or
 * switches turned off before "Next", are choices, not gaps to fill.
 */
export function skipAddsSources(audioDecided: boolean, sources: readonly AudioSource[]): boolean {
  return !audioDecided && sources.length === 0;
}

/** All switches off: Quick setup's "Next" then adds only what is turned on. */
export const NO_PICKS: QuickPicks = { output: false, input: false, discord: false };

/**
 * What the setup's audio step starts with. On an empty list the defaults are
 * on, as on first start. Run again over sources already set up, they start
 * off: a "Next" there is going on, not asking for the defaults beside them.
 */
export function setupPicks(sources: readonly AudioSource[]): QuickPicks {
  return sources.length === 0 ? QUICK_PICKS : NO_PICKS;
}
