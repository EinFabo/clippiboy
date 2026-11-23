import type { AudioDevice, AudioProcess, AudioSource } from "./types.ts";

export interface AppRow {
  process: AudioProcess;
  /** A source for this application exists already. */
  added: boolean;
  /** The application the game detection found. */
  isGame: boolean;
  /** The line under the name. */
  sub: string;
}

const same = (a: string | null | undefined, b: string | null | undefined) =>
  !!a && !!b && a.toLowerCase() === b.toLowerCase();

/** The Apps list of the add menu: what each row says and whether it can still be added. */
export function appRows(
  processes: AudioProcess[],
  sources: AudioSource[],
  detectedGame: string | null,
): AppRow[] {
  const known = sources.flatMap((s) =>
    s.kind.type === "process" && s.kind.mode === "include" ? [s.kind.exe] : [],
  );
  return processes.map((process) => {
    const isGame =
      !!detectedGame &&
      (same(process.name, detectedGame) ||
        same(process.exe.replace(/\.exe$/i, ""), detectedGame));
    const parts = [process.exe];
    if ((process.count ?? 1) > 1) parts.push(`${process.count} processes`);
    if (isGame) parts.push("Detected game");
    return {
      process,
      added: known.some((exe) => same(exe, process.exe)),
      isGame,
      sub: parts.join(" · "),
    };
  });
}

/**
 * Is this device a source already? A source that follows the Windows default
 * (`deviceId` "") takes the default device's row too: added again by its id,
 * the same sound would be in the clip twice.
 */
export function deviceAdded(
  sources: AudioSource[],
  type: "outputDevice" | "inputDevice",
  device: Pick<AudioDevice, "id" | "isDefault">,
): boolean {
  return sources.some(
    (s) =>
      s.kind.type === type &&
      "deviceId" in s.kind &&
      (s.kind.deviceId === device.id || (s.kind.deviceId === "" && device.isDefault)),
  );
}
