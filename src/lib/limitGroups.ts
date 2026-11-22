/**
 * The first `limit` items across a list of groups, in order; groups left with
 * nothing are dropped. Groups and items keep their identity when nothing is cut.
 */
export function limitGroups<T extends { items: unknown[] }>(groups: T[], limit: number): T[] {
  if (limit === Infinity) return groups;
  const out: T[] = [];
  let left = limit;
  for (const group of groups) {
    if (left <= 0) break;
    out.push(group.items.length <= left ? group : { ...group, items: group.items.slice(0, left) });
    left -= group.items.length;
  }
  return out;
}
