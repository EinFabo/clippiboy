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

/** What closing the player does: wait for a running write, ask about unapplied edits, or go. */
export function leaving(writing: boolean, dirty: boolean): "wait" | "ask" | "go" {
  if (writing) return "wait";
  return dirty ? "ask" : "go";
}
