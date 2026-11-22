import type { AudioDevice, AudioProcess, AudioSource } from "./types.ts";

/**
 * Sources saved with Solo on. The page has no Solo any more, and the core still
 * honours the flag — a stuck one would silence every other source in the
 * recordings with no way to undo it in the UI. These get written back as off.
 */
export function stuckSolo(sources: AudioSource[]): AudioSource[] {
  return sources.filter((s) => s.solo);
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
  /** The core's error / warning for this source, if any. */
  error?: string;
  warning?: string;
  /** The applications an output device is actually tapping right now. */
  tapped: string[];
  /** Output devices record only what individual applications leave over. */
  leftovers: boolean;
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
      if (kind.mode === "exclude") {
        status = {
          text: `Everything except ${found?.name ?? kind.exe ?? `PID ${kind.pid}`}`,
          tone: "plain",
        };
      } else {
        status = found
          ? { text: `${found.name} · running`, tone: "plain" }
          : { text: "Not running · waiting", tone: "warn" };
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
  if (status.tone === "plain" && ctx.warning) return { text: ctx.warning, tone: "warn" };
  return status;
}

/**
 * Is application capture refused on this machine? There is no flag and the
 * core has no message of its own for it: a Windows without process loopback
 * fails like any other source that cannot start ("audio source not available:
 * <HRESULT>"), which a source that is merely not running can produce too. So
 * only an error that names the Windows build counts; anything vaguer would
 * claim an old Windows for a plain missing app.
 */
export function appCaptureUnavailable(
  sources: AudioSource[],
  errors: Record<string, string>,
): boolean {
  return sources.some(
    (s) =>
      (s.kind.type === "game" || s.kind.type === "process") &&
      /\b20348\b/.test(errors[s.id] ?? ""),
  );
}

/** "+2.0 dB", "0.0 dB", "−3.5 dB". */
export function gainText(gainDb: number): string {
  return `${gainDb > 0 ? "+" : ""}${gainDb.toFixed(1)} dB`;
}
