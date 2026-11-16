import type { AudioProcess, AudioSource, SourceKind } from "./types.ts";

/** Discord in its three builds, lower case. Stable first: it wins when several run. */
const DISCORD_EXES = ["discord.exe", "discordptb.exe", "discordcanary.exe"];

export function isDiscordExe(exe: string | null | undefined): boolean {
  return !!exe && DISCORD_EXES.includes(exe.toLowerCase());
}

/** A Discord holding an audio session right now, stable before PTB before Canary. */
export function runningDiscord(processes: AudioProcess[]): AudioProcess | undefined {
  for (const exe of DISCORD_EXES) {
    const found = processes.find((p) => p.exe.toLowerCase() === exe);
    if (found) return found;
  }
  return undefined;
}

/** Is Discord on a track of its own already — switched on or not? */
export function hasDiscordSource(sources: AudioSource[]): boolean {
  return sources.some(
    (s) => s.kind.type === "process" && s.kind.mode === "include" && isDiscordExe(s.kind.exe),
  );
}

/** The source kind for that Discord, as the picker builds it from the same entry. */
export function discordKind(process: AudioProcess): SourceKind {
  return { type: "process", pid: process.pid, exe: process.exe, mode: "include" };
}

/**
 * What the note after the first clip says, if anything: put Discord on a
 * track of its own, or turn the one it has down. Nothing without a Discord to
 * speak of — then the note waits for a clip where there is one.
 */
export function discordHint(
  sources: AudioSource[],
  processes: AudioProcess[],
): "add" | "turnDown" | null {
  if (hasDiscordSource(sources)) return "turnDown";
  if (runningDiscord(processes)) return "add";
  return null;
}
