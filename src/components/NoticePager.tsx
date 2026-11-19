import { useEffect } from "react";
import { useNoticeBoard } from "@/lib/noticeBoard";
import { current, type NoticeId } from "@/lib/notices";

/**
 * One notice card shows at a time. Each card says here whether it wants to
 * show; `shown` is true for the most important one (or the one picked with the
 * pager), and `pager` is what goes at the left edge of its card — nothing while
 * it is the only one.
 */
export function useNoticeSlot(id: NoticeId, wants: boolean) {
  const set = useNoticeBoard((s) => s.set);
  useEffect(() => {
    set(id, wants);
    return () => set(id, false);
  }, [id, wants, set]);
  const wantsAll = useNoticeBoard((s) => s.wants);
  const picked = useNoticeBoard((s) => s.picked);
  const now = current(wantsAll, picked);
  // Right after a change the board lags a render behind: never show a card
  // that no longer wants to.
  const shown = wants && now?.id === id;
  return { shown, pager: shown && now && now.count > 1 ? <Pager index={now.index} count={now.count} /> : null };
}

function Pager({ index, count }: { index: number; count: number }) {
  const step = useNoticeBoard((s) => s.step);
  const arrow =
    "grid size-[26px] place-items-center rounded-full border border-line font-sans text-xs transition-colors duration-150 hover:bg-elevated hover:text-ink";
  return (
    <span className="mr-1 flex items-center gap-1.5 self-center border-r border-line pr-1.5 font-mono text-xs text-ink-muted">
      <button type="button" aria-label="Previous notice" className={arrow} onClick={() => step(-1)}>
        ‹
      </button>
      <span className="tabular-nums">
        {index + 1} / {count}
      </span>
      <button type="button" aria-label="Next notice" className={arrow} onClick={() => step(1)}>
        ›
      </button>
    </span>
  );
}
