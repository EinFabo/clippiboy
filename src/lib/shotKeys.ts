/**
 * The screenshot editor's keys. A letter picks a tool — from looking, too, it
 * opens the drawing — and the usual Ctrl keys undo, redo, save and copy.
 * Whoever calls this has already made sure no text field has the focus.
 */
export const TOOL_KEYS = {
  v: "select",
  a: "arrow",
  r: "box",
  e: "ellipse",
  p: "pen",
  t: "text",
  b: "blur",
} as const;

type ToolKey = keyof typeof TOOL_KEYS;

export type Shortcut =
  | { tool: (typeof TOOL_KEYS)[ToolKey] }
  | "undo"
  | "redo"
  | "save"
  | "copy";

interface KeyLike {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}

export function shortcut(event: KeyLike): Shortcut | null {
  // With Caps Lock on the key reads upper case.
  const k = event.key.toLowerCase();
  const ctrl = event.ctrlKey || event.metaKey;
  if (event.altKey) return null;
  if (ctrl) {
    if (k === "z") return event.shiftKey ? "redo" : "undo";
    if (event.shiftKey) return null;
    if (k === "y") return "redo";
    if (k === "s") return "save";
    if (k === "c") return "copy";
    return null;
  }
  if (k in TOOL_KEYS) return { tool: TOOL_KEYS[k as ToolKey] };
  return null;
}

/** The key that picks `tool`, for its button's tooltip. */
export function keyOf(tool: string): string | undefined {
  const found = Object.entries(TOOL_KEYS).find(([, t]) => t === tool);
  return found?.[0].toUpperCase();
}
