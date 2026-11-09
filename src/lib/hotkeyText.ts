/**
 * How a hotkey binding reads. The core stores one string per action:
 * `Ctrl+Shift+S`, `Mouse4`, `Ctrl+Mouse5`, `Pad:View+A`, any of them behind
 * `Hold:` when the action fires on holding rather than tapping
 * (`src-tauri/src/bindings.rs`). Old bindings are plain keyboard combinations
 * and read as before.
 */

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
  Mouse3: "Mouse 3",
  Mouse4: "Mouse 4",
  Mouse5: "Mouse 5",
};

/** What is printed on the key. The numpad follows a pattern, the rest is in
    {@link KEY_LABELS}; everything else is already called what it is called. */
export function keyLabel(key: string): string {
  const numpad = /^Numpad([0-9])$/.exec(key);
  if (numpad) return `Num ${numpad[1]}`;
  return KEY_LABELS[key] ?? key;
}

const HOLD = /^hold:\s*/i;
const PAD = /^pad:/i;

/** Fires on holding, not tapping. */
export function isHold(value: string): boolean {
  return HOLD.test(value.trim());
}

/** The binding without `Hold:` — what is pressed. */
export function triggerOf(value: string): string {
  return value.trim().replace(HOLD, "");
}

/** The same thing pressed, tapped or held. An empty binding stays empty. */
export function withHold(value: string, hold: boolean): string {
  const trigger = triggerOf(value);
  if (!trigger) return "";
  return hold ? `Hold:${trigger}` : trigger;
}

/** A controller combo rather than keys or a mouse button. */
export function isPad(value: string): boolean {
  return PAD.test(triggerOf(value));
}

/** The keys or buttons, each as it is printed. */
export function hotkeyKeys(value: string): string[] {
  const trigger = triggerOf(value);
  if (!trigger) return [];
  const keys = trigger.replace(PAD, "").split("+");
  return isPad(value) ? keys : keys.map(keyLabel);
}

/** The whole binding in one line: "Ctrl + Shift + S", "Hold Mouse 5". */
export function hotkeyLabel(value: string): string {
  const keys = hotkeyKeys(value).join(" + ");
  return isHold(value) && keys ? `Hold ${keys}` : keys;
}
