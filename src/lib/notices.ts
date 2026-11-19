/** The cards at the top of the main window, most important first. */
export const NOTICE_ORDER = ["ffmpeg", "update", "usage", "discord"] as const;
export type NoticeId = (typeof NOTICE_ORDER)[number];
export type Wants = Partial<Record<NoticeId, boolean>>;

/** Who wants to show right now, in priority order. */
export function queue(wants: Wants): NoticeId[] {
  return NOTICE_ORDER.filter((id) => wants[id]);
}

/** The card to show: the one picked with the pager while it still wants to
    show, else the most important. `index` is its place in the queue. */
export function current(
  wants: Wants,
  picked: NoticeId | null,
): { id: NoticeId; index: number; count: number } | null {
  const ids = queue(wants);
  if (ids.length === 0) return null;
  const at = picked ? ids.indexOf(picked) : -1;
  const index = at === -1 ? 0 : at;
  return { id: ids[index]!, index, count: ids.length };
}

/** The pick that stays once `id` starts wanting: a notice that ranks above the
    picked one takes the place back, the pick is dropped. */
export function pickAfterWant(picked: NoticeId | null, id: NoticeId): NoticeId | null {
  if (picked && NOTICE_ORDER.indexOf(id) < NOTICE_ORDER.indexOf(picked)) return null;
  return picked;
}

/** The neighbour in the queue, wrapping round at both ends. */
export function neighbour(wants: Wants, picked: NoticeId | null, dir: 1 | -1): NoticeId | null {
  const now = current(wants, picked);
  if (!now) return null;
  const ids = queue(wants);
  return ids[(now.index + dir + ids.length) % ids.length]!;
}
