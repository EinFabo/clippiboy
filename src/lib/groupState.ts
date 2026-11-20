/** The localStorage key one settings group keeps its open state under. */
export function groupKey(group: string): string {
  return `cb.settings.group.${group}`;
}

/** Open or closed as stored; anything else (or a throwing storage) is `fallback`. */
export function readOpen(group: string, fallback: boolean, storage?: Pick<Storage, "getItem">): boolean {
  try {
    const raw = (storage ?? localStorage).getItem(groupKey(group));
    return raw === "1" ? true : raw === "0" ? false : fallback;
  } catch {
    return fallback;
  }
}

export function writeOpen(group: string, open: boolean, storage?: Pick<Storage, "setItem">): void {
  try {
    (storage ?? localStorage).setItem(groupKey(group), open ? "1" : "0");
  } catch {
    /* the state then lasts until the page closes */
  }
}
