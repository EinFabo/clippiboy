import type { AudioDevice, AudioProcess, AudioSource } from "./types.ts";

/**
 * Sources saved with Solo on. The page has no Solo any more, and the core still
 * honours the flag — a stuck one would silence every other source in the
 * recordings with no way to undo it in the UI. These get written back as off.
 */
export function stuckSolo(sources: AudioSource[]): AudioSource[] {
  return sources.filter((s) => s.solo);
}

/** Writes of one source's Solo clean-up per session: the first and one retry. */
export const SOLO_CLEAR_ATTEMPTS = 2;

/**
 * Which stuck sources to write back as off right now. `attempts` counts the
 * writes started per source id; a source that reached the cap — or whose write
 * went through (the caller then sets its count to the cap) — is left alone, so
 * a core that keeps reporting Solo cannot make this write in a loop.
 */
export function soloToClear(
  sources: AudioSource[],
  attempts: ReadonlyMap<string, number>,
  max = SOLO_CLEAR_ATTEMPTS,
): AudioSource[] {
  return stuckSolo(sources).filter((s) => (attempts.get(s.id) ?? 0) < max);
}

/** How a status line reads: plain, a problem to look at, or a failure. */
export type StatusTone = "plain" | "warn" | "error";

export interface SourceStatus {
  text: string;
  tone: StatusTone;
}

export interface StatusContext {
  devices: AudioDevice[];
  processes: AudioProcess[];
  detectedGame: string | null;
  /** The core's error / warning (or mixer-only note) for this source, if any;
   *  a warning follows the source's name, see `noteLine`. */
  error?: string;
  warning?: string;
  /** The applications an output device is actually tapping right now. */
  tapped: string[];
  /** Output devices record only what individual applications leave over. */
  leftovers: boolean;
}

/**
 * Is anything recorded application by application?
 *
 * Then an output device records only what those leave over, otherwise the same
 * sound would be in the clip twice. Nothing to switch: it follows from the
 * sources, and the same rule runs in the core (`audio::records_single_applications`)
 * — including that without per-app capture (Windows 10) a device stays whole.
 */
export function leftoversMode(sources: AudioSource[], appCapture: boolean): boolean {
  return (
    appCapture &&
    sources.some(
      (s) =>
        s.enabled &&
        (s.kind.type === "game" || (s.kind.type === "process" && s.kind.mode === "include")),
    )
  );
}

/**
 * A core note on its own line. The notes are written to follow the source's
 * name ("Discord is not running — …"), the way the console and the Video page
 * show them; under the name in a row they start the line instead.
 */
export function noteLine(note: string): string {
  return note.charAt(0).toUpperCase() + note.slice(1);
}

const sameExe = (a: string | null | undefined, b: string | null | undefined) =>
  !!a && !!b && a.toLowerCase() === b.toLowerCase();

/**
 * The one line under a source's name: which application or device it is, and
 * whether it is there. A missing application or device is the warn tone, the
 * core's own error or warning (shortened by the row's width) beats both.
 */
export function sourceStatus(source: AudioSource, ctx: StatusContext): SourceStatus {
  if (ctx.error) return { text: ctx.error, tone: "error" };
  const kind = source.kind;
  let status: SourceStatus;
  switch (kind.type) {
    case "game":
      status = ctx.detectedGame
        ? { text: `${ctx.detectedGame} · running`, tone: "plain" }
        : { text: "No game running · waiting", tone: "warn" };
      break;
    case "process": {
      const found = ctx.processes.find((p) => p.pid === kind.pid || sameExe(p.exe, kind.exe));
      // Running means the core taps it, not that the list saw a session.
      const tappedNow = ctx.tapped[0];
      if (kind.mode === "exclude") {
        status = {
          text: `Everything except ${found?.name ?? kind.exe ?? `PID ${kind.pid}`}`,
          tone: "plain",
        };
      } else {
        status = tappedNow || found
          ? { text: `${found?.name ?? tappedNow} · running`, tone: "plain" }
          : { text: ctx.warning ? noteLine(ctx.warning) : "Not running · waiting", tone: "warn" };
      }
      break;
    }
    case "inputDevice":
    case "outputDevice": {
      // Nothing is known about the devices before the first list arrives.
      const missing =
        kind.deviceId !== "" &&
        ctx.devices.length > 0 &&
        !ctx.devices.some((d) => d.id === kind.deviceId);
      if (missing) {
        status = { text: "Unplugged", tone: "warn" };
        break;
      }
      const name =
        kind.deviceId === ""
          ? "Windows default"
          : (ctx.devices.find((d) => d.id === kind.deviceId)?.name ?? kind.deviceId);
      status = {
        text:
          kind.type === "outputDevice" && ctx.leftovers
            ? ctx.tapped.length > 0
              ? `${name} · ${ctx.tapped.join(", ")}`
              : `${name} · nothing left`
            : name,
        tone: "plain",
      };
      break;
    }
  }
  if (status.tone === "plain" && ctx.warning) return { text: noteLine(ctx.warning), tone: "warn" };
  return status;
}

/** "+2.0 dB", "0.0 dB", "−3.5 dB". */
export function gainText(gainDb: number): string {
  return `${gainDb > 0 ? "+" : ""}${gainDb.toFixed(1)} dB`;
}
