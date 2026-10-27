/**
 * What a text field should hand over when it is left: the trimmed draft, or
 * null when there is nothing to save (never edited, unchanged, or emptied —
 * a source keeps a name).
 */
export function labelToSave(draft: string | null, saved: string): string | null {
  if (draft === null) return null;
  const next = draft.trim();
  return next === "" || next === saved ? null : next;
}

/**
 * Whether letting go of a slider leaves its draft without a `change` event to
 * clear it: the browser fires none when the knob ends where it was pressed —
 * also after a drag that came back. `start` is the value at the press, `null`
 * without one.
 */
export function settlesWithoutChange(start: string | null, end: string): boolean {
  return start !== null && start === end;
}
