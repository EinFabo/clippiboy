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
