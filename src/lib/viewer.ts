// Key rules shared by the menu and the viewers. Pure, so they can be tested.

/** Whether an open menu consumes this key. Enter counts only with an entry picked. */
export function menuOwns(key: string, active: number): boolean {
  if (key === "Escape" || key === "ArrowUp" || key === "ArrowDown") return true;
  return key === "Enter" && active >= 0;
}

/** Whether the target takes typed text. Sliders do not: they keep the focus after a drag. */
export function typesText(target: { tagName?: string; type?: string } | null): boolean {
  if (!target) return false;
  if (target.tagName === "TEXTAREA") return true;
  return target.tagName === "INPUT" && target.type !== "range";
}

/** The keys a focused slider moves itself with. */
const SLIDER_KEYS = new Set(["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"]);

/**
 * Whether a focused slider keeps this key. Sliders are not text, so the
 * shortcuts reach them — but the arrows move a gain or the volume, and taken
 * for seeking they left a slider nobody could move by keyboard.
 */
export function sliderOwns(target: { tagName?: string; type?: string } | null, key: string): boolean {
  return target?.tagName === "INPUT" && target.type === "range" && SLIDER_KEYS.has(key);
}

/** What closing the player does: wait for a running write, ask about unapplied edits, or go. */
export function leaving(writing: boolean, dirty: boolean): "wait" | "ask" | "go" {
  if (writing) return "wait";
  return dirty ? "ask" : "go";
}

/**
 * What sharing from the player does first: refuse while a write runs (the
 * file is being replaced), go with a file that holds what is set, save what
 * is not applied yet — or ask first, for a cut that is final.
 */
export function beforeSharing(writing: boolean, unapplied: boolean, finalCut: boolean): "wait" | "go" | "save" | "ask" {
  if (writing) return "wait";
  if (!unapplied) return "go";
  return finalCut ? "ask" : "save";
}

/** Where a jump to `seconds` lands inside a file of `duration` — `null` while
 *  the length is not known yet (NaN before the metadata). */
export function seekTarget(seconds: number, duration: number): number | null {
  if (!Number.isFinite(seconds) || Number.isNaN(duration)) return null;
  return Math.min(Math.max(seconds, 0), duration);
}
