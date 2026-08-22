import { useCallback, useEffect, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import { useEngine } from "@/store";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Segmented, Toggle } from "@/components/ui/Controls";
import { api, events, inTauri } from "@/lib/ipc";
import { cn } from "@/lib/cn";
import type { OverlayCorner, UpdateInfo } from "@/lib/types";

const corners: [OverlayCorner, string][] = [
  ["topLeft", "top left"],
  ["topRight", "top right"],
  ["bottomLeft", "bottom left"],
  ["bottomRight", "bottom right"],
];

export function Settings() {
  const { config, targets, patchConfig, lastError } = useEngine();
  const monitors = targets.filter((t) => t.kind === "monitor");

  const patchOverlay = (patch: Partial<typeof config.overlay>) =>
    patchConfig({ overlay: { ...config.overlay, ...patch } });

  // Prefixed, so a monitor whose id happened to read "follow" could not pass
  // itself off as the setting that follows the game.
  const screen = config.overlay.followActiveScreen
    ? "follow"
    : (monitors.find(
        (m) =>
          config.overlay.monitor === m.id ||
          (config.overlay.monitor === null && m.isPrimary),
      )?.id ?? null);

  return (
    <div className="space-y-8 pb-12">
      <header className="pt-10">
        <h1 className="display text-4xl">Settings</h1>
      </header>

      <section>
        <SectionTitle title="Hotkeys" />
        <Hotkeys />
      </section>

      <section>
        <SectionTitle title="Behaviour" />
        <Card className="divide-y divide-line">
          <Row label="Start the buffer automatically">
            <Toggle
              checked={config.buffer.autoStart}
              onChange={(autoStart) =>
                patchConfig({ buffer: { ...config.buffer, autoStart } })
              }
            />
          </Row>
          <Row
            label="Only buffer in game"
            hint={
              config.buffer.autoStart
                ? "Starts once a game is in the foreground, stops half a minute after it exits"
                : "Needs automatic start"
            }
          >
            <Toggle
              checked={config.onlyBufferInGame}
              disabled={!config.buffer.autoStart}
              onChange={(onlyBufferInGame) => patchConfig({ onlyBufferInGame })}
            />
          </Row>
          <Row label="Start with Windows" hint="Starts hidden in the tray">
            <Toggle
              checked={config.autoStartWithWindows}
              onChange={(autoStartWithWindows) =>
                patchConfig({ autoStartWithWindows })
              }
            />
          </Row>
        </Card>
      </section>

      <section>
        <SectionTitle title="Banner over the game" />
        <Card className="divide-y divide-line">
          <Row
            label="Show banner"
            hint="A brief overlay above the game, like Medal or ShadowPlay"
          >
            <Toggle
              checked={config.overlay.enabled}
              onChange={(enabled) => patchOverlay({ enabled })}
            />
          </Row>
          <Row label="Clip saved">
            <Toggle
              checked={config.overlay.onClipSaved}
              disabled={!config.overlay.enabled}
              onChange={(onClipSaved) => patchOverlay({ onClipSaved })}
            />
          </Row>
          <Row label="Buffer on/off">
            <Toggle
              checked={config.overlay.onBufferToggle}
              disabled={!config.overlay.enabled}
              onChange={(onBufferToggle) => patchOverlay({ onBufferToggle })}
            />
          </Row>
          <Row label="Errors">
            <Toggle
              checked={config.overlay.onError}
              disabled={!config.overlay.enabled}
              onChange={(onError) => patchOverlay({ onError })}
            />
          </Row>
          <Row label="Screen">
            <Segmented
              className="flex-wrap justify-end"
              disabled={!config.overlay.enabled}
              value={screen}
              options={[
                ...monitors.map((monitor) => ({
                  key: monitor.id,
                  label: monitor.title.split("—")[0].trim(),
                })),
                { key: "follow", label: "follows the game" },
              ]}
              onChange={(key) =>
                patchOverlay(
                  key === "follow"
                    ? { followActiveScreen: true }
                    : { monitor: key, followActiveScreen: false },
                )
              }
            />
          </Row>
          <Row label="Corner">
            <Segmented
              disabled={!config.overlay.enabled}
              value={config.overlay.corner}
              options={corners.map(([corner, label]) => ({ key: corner, label }))}
              onChange={(corner) => patchOverlay({ corner })}
            />
          </Row>
          <Row label="Duration" hint="Errors always stay at least 6 s">
            <Segmented
              disabled={!config.overlay.enabled}
              value={String(config.overlay.durationMs)}
              options={[2000, 3500, 5000, 8000].map((ms) => ({
                key: String(ms),
                label: `${ms / 1000} s`,
              }))}
              onChange={(key) => patchOverlay({ durationMs: Number(key) })}
            />
          </Row>
        </Card>
        <p className="mt-3 text-xs text-ink-faint">
          The banner cannot appear over a game in exclusive fullscreen — ClippiBoy
          deliberately does not hook into the game process. Borderless fullscreen
          and windowed mode work.
        </p>
      </section>

      <section>
        <SectionTitle title="Storage location" />
        <ClipDir />
      </section>

      <section>
        <SectionTitle title="Version" />
        <Updates />
      </section>

      {lastError && (
        <Card className="border-live/40 bg-live/8 p-4 text-sm text-live">
          {lastError}
        </Card>
      )}

    </div>
  );
}

function Row({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-6 p-5">
      <div className="min-w-0">
        <p className="text-sm font-medium">{label}</p>
        {hint && <p className="mt-1 text-xs text-ink-muted">{hint}</p>}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

/**
 * Display name of a key. What gets stored is always the spelling the core's
 * shortcut parser understands — this only holds what is printed on the keyboard.
 * Keys whose name already matches are left out.
 */
const KEY_LABELS: Record<string, string> = {
  Super: "Win",
  Escape: "Esc",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  PageUp: "Page ↑",
  PageDown: "Page ↓",
  PrintScreen: "PrtSc",
  Delete: "Del",
  Insert: "Ins",
  CapsLock: "Caps",
  ScrollLock: "Scroll",
  NumLock: "Num",
  NumpadAdd: "Num +",
  NumpadSubtract: "Num −",
  NumpadMultiply: "Num ×",
  NumpadDivide: "Num ÷",
  NumpadDecimal: "Num .",
  NumpadEnter: "Num Enter",
  NumpadEqual: "Num =",
};

/** What is printed on the key. The numpad follows a pattern, the rest is in
    {@link KEY_LABELS}; everything else is already called what it is called. */
function keyLabel(key: string): string {
  const numpad = /^Numpad([0-9])$/.exec(key);
  if (numpad) return `Num ${numpad[1]}`;
  return KEY_LABELS[key] ?? key;
}

/**
 * Keys the core's hotkey parser knows and that are not already recognized by
 * their pattern (letters, digits, F keys, numpad).
 *
 * Whatever is missing here the core will not accept — the context menu key, say,
 * or the small `<` key next to the left shift. Recording then simply carries on
 * instead of offering an assignment that would be rejected right away.
 */
const KEYS = new Set([
  "Backquote", "Backslash", "BracketLeft", "BracketRight", "Comma", "Equal",
  "Minus", "Period", "Quote", "Semicolon", "Slash",
  "Backspace", "CapsLock", "Enter", "Space", "Tab",
  "Delete", "End", "Home", "Insert", "PageDown", "PageUp",
  "PrintScreen", "ScrollLock", "Pause", "NumLock",
  "ArrowDown", "ArrowLeft", "ArrowRight", "ArrowUp",
  "NumpadAdd", "NumpadDecimal", "NumpadDivide", "NumpadEnter", "NumpadEqual",
  "NumpadMultiply", "NumpadSubtract",
  "AudioVolumeDown", "AudioVolumeUp", "AudioVolumeMute",
  "MediaPlayPause", "MediaStop", "MediaTrackNext", "MediaTrackPrevious",
]);

function supported(code: string): boolean {
  return (
    /^(Key[A-Z]|Digit[0-9]|Numpad[0-9]|F([1-9]|1[0-9]|2[0-4]))$/.test(code) ||
    KEYS.has(code)
  );
}

/**
 * Turn a key press into the spelling the core accepts.
 *
 * Modifiers are allowed but not required: whoever wants to bind `F9` on its own
 * should be able to — that is how the other recorders do it too.
 */
function accelerator(event: KeyboardEvent): string | null {
  const mods: string[] = [];
  if (event.ctrlKey) mods.push("Ctrl");
  if (event.altKey) mods.push("Alt");
  if (event.shiftKey) mods.push("Shift");
  if (event.metaKey) mods.push("Super");

  const code = event.code;
  // A modifier alone is not an assignment yet — let them keep typing.
  if (/^(Control|Alt|Shift|Meta|OS)(Left|Right)$/.test(code)) return null;
  if (!supported(code)) return null;

  // `event.code` almost everywhere already matches the parser; only letter and
  // digit keys are usually written in short form.
  const key = /^Key[A-Z]$/.test(code)
    ? code.slice(3)
    : /^Digit[0-9]$/.test(code)
      ? code.slice(5)
      : code;
  return [...mods, key].join("+");
}

/** Modifiers that mark an assignment as "emergencies only". */
const MODIFIERS = ["Ctrl", "Alt", "Shift", "Super"];

/** Keys that would not type anything in a text field anyway. */
const HARMLESS =
  /^(F([1-9]|1[0-9]|2[0-4])|PrintScreen|ScrollLock|Pause|NumLock|CapsLock|Insert|AudioVolume|Media)/;

/**
 * An assignment that gets in the way while typing: a single key that has a job
 * in a text field too. `F9` or `PrtSc` are harmless, a bare `S` is not.
 */
function risky(value: string): boolean {
  const keys = value.split("+");
  if (keys.some((key) => MODIFIERS.includes(key))) return false;
  return !HARMLESS.test(keys[0] ?? "");
}

function Hotkeys() {
  const { config, setHotkeys } = useEngine();
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const warn =
    risky(config.saveClipHotkey) || risky(config.toggleBufferHotkey);

  const apply = async (saveClip: string, toggleBuffer: string) => {
    setSaving(true);
    setError(null);
    try {
      await setHotkeys(saveClip, toggleBuffer);
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      <Card className="divide-y divide-line">
        <Row label="Save clip">
          <HotkeyInput
            value={config.saveClipHotkey}
            busy={saving}
            onChange={(value) => apply(value, config.toggleBufferHotkey)}
          />
        </Row>
        <Row label="Buffer on/off">
          <HotkeyInput
            value={config.toggleBufferHotkey}
            busy={saving}
            onChange={(value) => apply(config.saveClipHotkey, value)}
          />
        </Row>
      </Card>
      <p className="mt-3 text-xs text-ink-faint">
        Click and press the key you want — with or without Ctrl, Alt, Shift and
        the Windows key. Escape cancels.
      </p>
      {warn && (
        <p className="mt-2 text-xs text-ink-muted">
          A single key you can also type with applies everywhere: it fires in the
          middle of a chat message. F keys and PrtSc are the quieter spots.
        </p>
      )}
      {error && <p className="mt-2 text-xs text-live">{error}</p>}
    </>
  );
}

/** Shows a combination and records a new one on click. */
function HotkeyInput({
  value,
  busy,
  onChange,
}: {
  value: string;
  busy?: boolean;
  onChange: (value: string) => void;
}) {
  const [recording, setRecording] = useState(false);
  // Otherwise the handler would hold the value from when recording started.
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  // Did recording end with a new combination? Then the core re-registers the
  // hotkeys itself, and a `resume` here would get in its way.
  const committed = useRef(false);

  const stop = useCallback(() => setRecording(false), []);

  useEffect(() => {
    if (!recording) return;
    committed.current = false;
    if (inTauri) void api.suspendHotkeys();
    const onKeyDown = (event: KeyboardEvent) => {
      // While recording, every keystroke belongs here — including Tab and Enter,
      // which would otherwise travel through the UI.
      event.preventDefault();
      event.stopPropagation();
      if (event.code === "Escape") {
        stop();
        return;
      }
      const next = accelerator(event);
      if (!next) return;
      // Even an unchanged combination goes through the core: that re-registers
      // the suspended hotkeys along the way.
      committed.current = true;
      stop();
      onChangeRef.current(next);
    };
    window.addEventListener("keydown", onKeyDown, true);
    // A click elsewhere or a window switch also ends recording.
    window.addEventListener("mousedown", stop);
    window.addEventListener("blur", stop);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("mousedown", stop);
      window.removeEventListener("blur", stop);
      if (inTauri && !committed.current) void api.resumeHotkeys();
    };
  }, [recording, stop]);

  return (
    <button
      type="button"
      disabled={busy}
      onMouseDown={(event) => {
        // Without this our own click would end recording again immediately.
        event.stopPropagation();
        setRecording((on) => !on);
      }}
      className={cn(
        "flex items-center gap-1.5 rounded-pill border px-2 py-1.5 transition-colors duration-150",
        "disabled:pointer-events-none disabled:opacity-40",
        recording
          ? "border-accent bg-accent/10"
          : "border-transparent hover:border-line hover:bg-elevated",
      )}
    >
      {recording ? (
        <span className="px-1.5 text-[13px] text-accent">Press a key …</span>
      ) : (
        <Hotkey value={value} />
      )}
    </button>
  );
}

function Hotkey({ value }: { value: string }) {
  return (
    <div className="flex gap-1.5">
      {value.split("+").map((key) => (
        <kbd
          key={key}
          className="rounded-inner border border-line bg-elevated px-2.5 py-1 font-mono text-xs text-ink-muted"
        >
          {keyLabel(key)}
        </kbd>
      ))}
    </div>
  );
}

/** Folder for new clips — pick, reset, open. */
function ClipDir() {
  const { config, setClipDir } = useEngine();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Only for the "Reset" button: without the comparison the UI would not know
  // whether there is anything to reset at all.
  const [fallback, setFallback] = useState<string | null>(null);

  useEffect(() => {
    if (!inTauri) return;
    api.defaultClipDir().then(setFallback).catch(() => {});
  }, []);

  const apply = async (dir: string) => {
    setBusy(true);
    setError(null);
    try {
      await setClipDir(dir);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  const pick = async () => {
    setError(null);
    let picked: string | string[] | null;
    try {
      picked = await openDialog({
        directory: true,
        multiple: false,
        defaultPath: config.clipDir,
        title: "Folder for new clips",
      });
    } catch (err) {
      setError(String(err));
      return;
    }
    // Cancelling the dialog yields null.
    if (typeof picked !== "string") return;
    await apply(picked);
  };

  const canReset = fallback !== null && fallback !== config.clipDir;

  return (
    <>
      <Card className="flex items-center justify-between gap-6 p-5">
        <p className="truncate font-mono text-sm text-ink-muted" title={config.clipDir}>
          {config.clipDir}
        </p>
        <div className="flex shrink-0 gap-2">
          {canReset && (
            <Button
              size="sm"
              variant="ghost"
              disabled={busy}
              onClick={() => apply(fallback)}
            >
              Reset
            </Button>
          )}
          <Button size="sm" variant="secondary" disabled={!inTauri || busy} onClick={pick}>
            Change
          </Button>
        </div>
      </Card>
      <p className="mt-3 text-xs text-ink-faint">
        Applies to new clips. Ones already saved stay where they are and remain
        playable.
      </p>
      {error && <p className="mt-2 text-xs text-live">{error}</p>}
    </>
  );
}

/** Version, update check and installation. */
function Updates() {
  const [version, setVersion] = useState("0.1.0");
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  // `null` means: not checked yet. Otherwise the text under the row.
  const [state, setState] = useState<"idle" | "checking" | "current" | "failed">(
    "idle",
  );
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!inTauri) return;
    api.appVersion().then(setVersion).catch(() => {});
    // Startup checks by itself; the result arrives as an event.
    let unlisten: (() => void) | undefined;
    events.onUpdateAvailable(setUpdate).then((fn) => (unlisten = fn));
    return () => unlisten?.();
  }, []);

  const check = async () => {
    setState("checking");
    setError(null);
    try {
      const found = await api.checkUpdate();
      setUpdate(found);
      setState(found ? "idle" : "current");
    } catch (err) {
      setState("failed");
      setError(String(err));
    }
  };

  const install = async () => {
    setInstalling(true);
    setError(null);
    try {
      // Does not come back: Windows quits the app and starts the installer.
      await api.installUpdate();
    } catch (err) {
      setInstalling(false);
      setError(String(err));
    }
  };

  return (
    <>
      <Card className="divide-y divide-line">
        <Row
          label={`ClippiBoy ${version}`}
          hint={
            update
              ? `Version ${update.version} is available`
              : state === "current"
                ? "Up to date"
                : state === "failed"
                  ? "The update check failed"
                  : "Updates come from GitHub Releases and are signed"
          }
        >
          <div className="flex gap-2">
            <Button
              size="sm"
              variant="secondary"
              disabled={!inTauri || state === "checking" || installing}
              onClick={check}
            >
              {state === "checking" ? "Checking …" : "Check for updates"}
            </Button>
            {update && (
              <Button size="sm" variant="primary" disabled={installing} onClick={install}>
                {installing ? "Installing …" : `Update to ${update.version}`}
              </Button>
            )}
          </div>
        </Row>
        {update?.notes && (
          <div className="p-5">
            <p className="text-xs whitespace-pre-line text-ink-muted">
              {update.notes}
            </p>
          </div>
        )}
      </Card>
      {update && (
        <p className="mt-3 text-xs text-ink-faint">
          Installing quits ClippiBoy and runs the installer — a running buffer is
          stopped cleanly first.
        </p>
      )}
      {error && <p className="mt-3 text-xs text-live">{error}</p>}
    </>
  );
}
