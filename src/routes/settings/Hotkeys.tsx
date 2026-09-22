import { useCallback, useEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card, SectionTitle } from "@/components/ui/Card";
import { api, inTauri } from "@/lib/ipc";
import { cn } from "@/lib/cn";
import { Row } from "./shared";
import { StreamDeck } from "./StreamDeck";

/**
 * How things get triggered: the global hotkeys, and the Stream Deck, which
 * presses the same buttons from outside.
 */
export function HotkeysTab() {
  return (
    <>
      <section>
        <SectionTitle title="Hotkeys" />
        <Hotkeys />
      </section>

      <section>
        <SectionTitle title="Stream Deck" />
        <StreamDeck />
      </section>
    </>
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
  const { config, setHotkeys } = useEngine(
    useShallow((s) => ({ config: s.config, setHotkeys: s.setHotkeys })),
  );
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const warn =
    risky(config.saveClipHotkey) ||
    risky(config.toggleBufferHotkey) ||
    risky(config.screenshotHotkey) ||
    risky(config.recordHotkey) ||
    risky(config.consoleHotkey);

  const apply = async (
    saveClip: string,
    toggleBuffer: string,
    screenshot: string,
    record: string,
    consoleKey: string,
  ) => {
    setSaving(true);
    setError(null);
    try {
      await setHotkeys(saveClip, toggleBuffer, screenshot, record, consoleKey);
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
            onChange={(value) =>
              apply(
                value,
                config.toggleBufferHotkey,
                config.screenshotHotkey,
                config.recordHotkey,
                config.consoleHotkey,
              )
            }
          />
        </Row>
        <Row label="Buffer on/off">
          <HotkeyInput
            value={config.toggleBufferHotkey}
            busy={saving}
            onChange={(value) =>
              apply(
                config.saveClipHotkey,
                value,
                config.screenshotHotkey,
                config.recordHotkey,
                config.consoleHotkey,
              )
            }
          />
        </Row>
        <Row label="Screenshot" hint="Info: Works without a running buffer">
          <HotkeyInput
            value={config.screenshotHotkey}
            busy={saving}
            onChange={(value) =>
              apply(
                config.saveClipHotkey,
                config.toggleBufferHotkey,
                value,
                config.recordHotkey,
                config.consoleHotkey,
              )
            }
          />
        </Row>
        <Row label="Recording start/stop" hint="Info: Brings the capture up by itself">
          <HotkeyInput
            value={config.recordHotkey}
            busy={saving}
            onChange={(value) =>
              apply(
                config.saveClipHotkey,
                config.toggleBufferHotkey,
                config.screenshotHotkey,
                value,
                config.consoleHotkey,
              )
            }
          />
        </Row>
        <Row
          label="Konsole über dem Spiel"
          hint="Info: Geht nicht über Spielen im exklusiven Vollbild"
        >
          <HotkeyInput
            value={config.consoleHotkey}
            busy={saving}
            onChange={(value) =>
              apply(
                config.saveClipHotkey,
                config.toggleBufferHotkey,
                config.screenshotHotkey,
                config.recordHotkey,
                value,
              )
            }
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