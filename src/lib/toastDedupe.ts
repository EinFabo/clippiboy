/** A toast with the same kind and message inside this window counts as a repeat. */
export const TOAST_DEDUPE_MS = 1500;

/**
 * Whether a toast is a duplicate of one shown just before. `last` is the
 * previous toast (kind, message, time) or null.
 */
export function isDuplicateToast(
  last: { kind: string; message: string; at: number } | null,
  next: { kind: string; message: string },
  now: number,
): boolean {
  return (
    last !== null && last.kind === next.kind && last.message === next.message && now - last.at < TOAST_DEDUPE_MS
  );
}
