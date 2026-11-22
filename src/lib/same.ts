/** Whether two string maps hold the same keys with the same values. */
export function sameRecord(a: Record<string, string>, b: Record<string, string>): boolean {
  const keys = Object.keys(a);
  if (keys.length !== Object.keys(b).length) return false;
  return keys.every((k) => Object.prototype.hasOwnProperty.call(b, k) && a[k] === b[k]);
}

/**
 * Whether two JSON-like values hold the same data: plain objects and arrays are
 * compared by content, everything else by `Object.is`.
 *
 * For data read back from the core, where an unchanged record arrives as a new
 * object — the gallery uses it to leave such a clip's tile alone.
 */
export function sameData(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) return false;
    return a.every((item, i) => sameData(item, b[i]));
  }
  if (Array.isArray(b)) return false;
  const x = a as Record<string, unknown>;
  const y = b as Record<string, unknown>;
  const keys = Object.keys(x);
  if (keys.length !== Object.keys(y).length) return false;
  return keys.every((k) => Object.prototype.hasOwnProperty.call(y, k) && sameData(x[k], y[k]));
}

/**
 * The part of `patch` that differs from `current`, or `null` when nothing does.
 * Lets a store skip a `set` for a tick whose data came back unchanged.
 */
export function changedPart<T extends object>(current: T, patch: Partial<T>): Partial<T> | null {
  let out: Partial<T> | null = null;
  for (const k of Object.keys(patch) as (keyof T)[]) {
    if (!sameData(current[k], patch[k])) (out ??= {} as Partial<T>)[k] = patch[k];
  }
  return out;
}
