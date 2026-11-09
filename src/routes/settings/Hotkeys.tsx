import { useCallback, useEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card, SectionTitle } from "@/components/ui/Card";
import { IconClose } from "@/components/icons";
import { Segmented } from "@/components/ui/Controls";
import { api, events, inTauri } from "@/lib/ipc";
import { cn } from "@/lib/cn";
import type { HotkeyField } from "@/lib/types";
import { hotkeyKeys, isHold, isPad, triggerOf, withHold } from "@/lib/hotkeyText";
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
  if (!value || isPad(value)) return false;
  const keys = triggerOf(value).split("+");
  if (/^Mouse[345]$/.test(keys[keys.length - 1] ?? "")) return false;
  if (keys.some((key) => MODIFIERS.includes(key))) return false;
  return !HARMLESS.test(keys[0] ?? "");
}

/** One row per action: the label, its first binding and its second. */
const ROWS: Array<{ label: string; hint?: string; first: HotkeyField; second: HotkeyField }> = [
  { label: "Save clip", first: "saveClipHotkey", second: "saveClipHotkeyAlt" },
  { label: "Buffer on/off", first: "toggleBufferHotkey", second: "toggleBufferHotkeyAlt" },
  {
    label: "Screenshot",
    hint: "Info: Works without a running buffer",
    first: "screenshotHotkey",
    second: "screenshotHotkeyAlt",
  },
  {
    label: "Recording start/stop",
    hint: "Info: Brings the capture up by itself",
    first: "recordHotkey",
    second: "recordHotkeyAlt",
  },
  {
    label: "Console over the game",
    hint: "Does not work over games in exclusive fullscreen",
    first: "consoleHotkey",
    second: "consoleHotkeyAlt",
  },
];

export function Hotkeys() {
  const { config, setHotkeys } = useEngine(
    useShallow((s) => ({ config: s.config, setHotkeys: s.setHotkeys })),
  );
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  // One field records at a time — starting another ends the first.
  const [recording, setRecording] = useState<HotkeyField | null>(null);
  const warn = ROWS.some(({ first, second }) => risky(config[first]) || risky(config[second]));

  const apply = async (field: HotkeyField, value: string) => {
    setSaving(true);
    setError(null);
    try {
      await setHotkeys({ [field]: value });
    } catch (err) {
      setError(String(err));
      // The core puts the hotkeys back itself after a rejected set; this is
      // for a call that never got there.
      if (inTauri) void api.resumeHotkeys();
    } finally {
      setSaving(false);
    }
  };

  const input = (field: HotkeyField, second: boolean) => (
    <HotkeyInput
      value={config[field]}
      second={second}
      busy={saving}
      recording={recording === field}
      onRecording={(on) => setRecording((now) => (on ? field : now === field ? null : now))}
      onError={setError}
      onChange={(value) => apply(field, value)}
    />
  );

  return (
    <>
      <Card className="divide-y divide-line">
        {ROWS.map(({ label, hint, first, second }) => (
          <Row key={first} label={label} hint={hint}>
            <div className="flex flex-col items-end gap-1">
              {input(first, false)}
              {input(second, true)}
            </div>
          </Row>
        ))}
      </Card>
      <p className="mt-3 text-xs text-ink-faint">
        Click, then press a key, a mouse button (middle or side) or a controller
        combo. Escape cancels, × unbinds.
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

/** Shows a combination and records a new one on click — a key, a mouse
    button or a controller combo. Tap or hold sits beside it; the × unbinds:
    an empty value is an action without a hotkey. */
function HotkeyInput({
  value,
  second,
  busy,
  recording,
  onRecording,
  onChange,
  onError,
}: {
  value: string;
  /** The second binding: smaller, and "Add" while empty. */
  second?: boolean;
  busy?: boolean;
  recording: boolean;
  onRecording: (on: boolean) => void;
  onChange: (value: string) => void;
  onError: (message: string | null) => void;
}) {
  const onRecordingRef = useRef(onRecording);
  onRecordingRef.current = onRecording;
  // Otherwise the handler would hold the value from when recording started.
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;
  // A new binding keeps tap or hold as it was.
  const hold = isHold(value);
  const holdRef = useRef(hold);
  holdRef.current = hold;
  // Did recording end with a new combination? Then the core re-registers the
  // hotkeys itself, and a `resume` here would get in its way.
  const committed = useRef(false);

  const stop = useCallback(() => onRecordingRef.current(false), []);

  useEffect(() => {
    if (!recording) return;
    committed.current = false;
    const commit = (next: string) => {
      // Even an unchanged combination goes through the core: that re-registers
      // the suspended hotkeys along the way.
      committed.current = true;
      stop();
      onChangeRef.current(withHold(next, holdRef.current));
    };
    if (inTauri) {
      void api.suspendHotkeys();
      void api.captureInput(true);
    }
    // Mouse buttons and controllers come from the core: the page sees a
    // controller not at all, and a mouse button only over the window.
    let unlisten: (() => void) | null = null;
    let gone = false;
    if (inTauri) {
      void events
        .onInputCaptured(({ binding, error }) => {
          if (binding) commit(binding);
          else if (error) onErrorRef.current(error);
          // Neither: the core ended recording (timed out, focus gone) and
          // has put the hotkeys back already.
          else {
            committed.current = true;
            stop();
          }
        })
        .then((off) => {
          if (gone) off();
          else unlisten = off;
        });
    }
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
      if (next) commit(next);
    };
    // A click elsewhere or a window switch also ends recording — the left or
    // right button; middle and side buttons are what is being recorded.
    const onMouseDown = (event: MouseEvent) => {
      if (event.button === 0 || event.button === 2) stop();
      // A middle click would start the WebView's autoscroll on top.
      else if (event.button === 1) event.preventDefault();
    };
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("mousedown", onMouseDown);
    window.addEventListener("blur", stop);
    return () => {
      gone = true;
      unlisten?.();
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("mousedown", onMouseDown);
      window.removeEventListener("blur", stop);
      if (inTauri) {
        void api.captureInput(false);
        if (!committed.current) void api.resumeHotkeys();
      }
    };
  }, [recording, stop]);

  return (
    <div className="flex items-center gap-1">
      {value && !recording && (
        <Segmented
          value={hold ? "hold" : "tap"}
          options={[
            { key: "tap", label: "Tap" },
            { key: "hold", label: "Hold" },
          ]}
          onChange={(mode) => {
            if ((mode === "hold") !== hold) onChange(withHold(value, mode === "hold"));
          }}
          disabled={busy}
          className="mr-1"
        />
      )}
      <button
        type="button"
        disabled={busy}
        onMouseDown={(event) => {
          // Without this our own click would end recording again immediately.
          event.stopPropagation();
          if (event.button === 0) onRecording(!recording);
        }}
        className={cn(
          "flex items-center gap-1.5 rounded-pill border transition-colors duration-150",
          second ? "px-1.5 py-1" : "px-2 py-1.5",
          "disabled:pointer-events-none disabled:opacity-40",
          recording
            ? "border-accent bg-accent/10"
            : "border-transparent hover:border-line hover:bg-elevated",
        )}
      >
        {recording ? (
          <span className="px-1.5 text-[13px] text-accent">Press a key or button …</span>
        ) : value ? (
          <Hotkey value={value} small={second} />
        ) : second ? (
          <span className="px-1 text-xs text-ink-faint">+ Add</span>
        ) : (
          <span className="px-1.5 text-[13px] text-ink-faint">Not set</span>
        )}
      </button>
      {value && !recording && (
        <button
          type="button"
          disabled={busy}
          title="Unbind"
          aria-label="Unbind"
          onClick={() => onChange("")}
          className={cn(
            "grid size-7 place-items-center rounded-pill text-ink-faint transition-colors duration-150",
            "hover:bg-elevated hover:text-ink",
            "disabled:pointer-events-none disabled:opacity-40",
          )}
        >
          <IconClose className="size-3.5" />
        </button>
      )}
    </div>
  );
}

function Hotkey({ value, small }: { value: string; small?: boolean }) {
  const pad = isPad(value);
  return (
    <div className="flex items-center gap-1.5">
      {pad && <span className="text-[11px] uppercase tracking-wide text-ink-faint">Pad</span>}
      {hotkeyKeys(value).map((key) => (
        <kbd
          key={key}
          className={cn(
            "border border-line bg-elevated font-mono text-ink-muted",
            small ? "px-2 py-0.5 text-[11px]" : "px-2.5 py-1 text-xs",
            pad ? "rounded-pill" : "rounded-inner",
          )}
        >
          {key}
        </kbd>
      ))}
    </div>
  );
}
