import {
  type PointerEvent as ReactPointerEvent,
  type RefObject,
  useEffect,
  useLayoutEffect,
  useRef,
} from "react";
import { EASE_OUT_SOFT, prefersReducedMotion } from "./motion";
import { useFlip } from "./useFlip";
import { insertIndex, landingShift, moveTo, scrollStep, shiftOf } from "./reorder";

/** Rows that take part carry this attribute with their id — the same as `useFlip`'s. */
const KEY = "data-flip";
/**
 * An optional, hidden element in the container that marks the slot the card
 * would land in while it is up. The container has to be positioned for it.
 */
const SLOT = "data-reorder-slot";
/** How far the pointer has to travel before a press on the grip becomes a lift. */
const THRESHOLD_PX = 4;
/** How long the other rows take to step aside, and the card to settle. */
const SLIDE_MS = 180;
/** The lifted card grows this much, as if it came a little closer. */
const LIFT_SCALE = 1.01;
/** How long a dropped order may take to come back from the core before the rows go home. */
const WRITE_TIMEOUT_MS = 5000;

/** What is measured once when the card comes up; the layout does not change during a drag. */
interface Lift {
  rows: HTMLElement[];
  card: HTMLElement;
  from: number;
  /** Tops and heights of every row, in the viewport as it was when the drag began. */
  tops: number[];
  heights: number[];
  /** The middles of the other rows — what the card's middle is compared with. */
  mids: number[];
  /** How far a row steps aside: the card's height plus the gap. */
  step: number;
  /** The card stays within the list: from the first row's top to the last row's bottom. */
  minDy: number;
  maxDy: number;
  /** The slot the card is over right now (`insertIndex`). */
  slot: number;
  scroller: HTMLElement | null;
  scroll0: number;
  /** Where auto-scrolling starts, in the viewport. */
  edgeTop: number;
  edgeBottom: number;
  frame: number;
}

/** One press on a grip, from pointer down to pointer up. */
interface Session {
  pointerId: number;
  id: string;
  startX: number;
  startY: number;
  clientY: number;
  /** Set once the pointer has travelled far enough and the card is up. */
  lift: Lift | null;
  /** Escape, or nothing to lift: the rest of this press is ignored. */
  done: boolean;
  onKey: (event: KeyboardEvent) => void;
  onScroll: () => void;
  userSelect: string;
}

/** The props for a row's grip. */
export interface GripProps {
  onPointerDown: (event: ReactPointerEvent<HTMLElement>) => void;
  onPointerMove: (event: ReactPointerEvent<HTMLElement>) => void;
  onPointerUp: (event: ReactPointerEvent<HTMLElement>) => void;
  onPointerCancel: (event: ReactPointerEvent<HTMLElement>) => void;
  onLostPointerCapture: (event: ReactPointerEvent<HTMLElement>) => void;
  style: { touchAction: "none" };
}

/**
 * Lift a row by its grip and put it down elsewhere in the list.
 *
 * The card comes up (`data-lifted` on the row, styled by the page) and follows
 * the pointer; the others step aside so a gap opens where it would land. All of
 * that is transforms written straight to the rows — no render per pointer move,
 * and the DOM order stays as it is until the new order is written, once, on
 * release. The rows keep standing where they were shown until that order comes
 * back (from the core, in the app) and then trade their transforms for the new
 * layout without moving a pixel; `useFlip` holds still for that one change,
 * which it would otherwise animate a second time from the old places. Every
 * other change of the order — the arrow keys on the grip, a source added —
 * still travels with `useFlip`, which this hook runs for the list.
 *
 * Escape or a cancelled pointer puts everything back and writes nothing. Near
 * the top or bottom of the scroll area the page scrolls along. With reduced
 * motion the card still follows the pointer, but nothing slides.
 */
export function useReorder({
  container,
  ids,
  onMove,
}: {
  container: RefObject<HTMLElement | null>;
  /** The rows' ids in their order, as the `data-flip` attributes carry them. */
  ids: string[];
  /** Called once per drop, with the new order and the row that moved, and only when it changed. */
  onMove: (ids: string[], moved: string) => void;
}): { grip: (id: string) => GripProps } {
  const signature = ids.join(",");
  const move = useRef(onMove);
  const session = useRef<Session | null>(null);
  /**
   * A dropped order on its way to the core, with where each row stood in the
   * layout when it was dropped — by the time the order comes back, React has
   * already laid them out anew.
   */
  const pending = useRef<{ timer: number; places: Map<string, number> } | null>(null);
  /** Running slides, so a new one can start from where the old one is. */
  const moving = useRef(new WeakMap<HTMLElement, Animation>());
  const hold = useRef(false);

  useLayoutEffect(() => {
    move.current = onMove;
  });

  // Before the effect below: it has to see `hold` while it is still set.
  useFlip(container, signature, 260, hold);

  useLayoutEffect(() => {
    const waiting = pending.current;
    if (waiting) {
      window.clearTimeout(waiting.timer);
      pending.current = null;
      settle(rowsOf(container.current), waiting.places);
    } else if (session.current?.lift) {
      // The list changed under the card (a source added or removed by the
      // core): what was measured no longer holds. Put it down where it is.
      const s = session.current;
      for (const row of s.lift!.rows) clear(row);
      hideSlot();
      finish(s);
    }
    hold.current = false;
    // Only a new order matters here; the refs are read when it comes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signature]);

  useEffect(
    () => () => {
      if (session.current) finish(session.current);
      if (pending.current) window.clearTimeout(pending.current.timer);
    },
    [],
  );

  /** End a row's running slide; whoever starts the next reads where it is shown first. */
  function stop(row: HTMLElement) {
    moving.current.get(row)?.cancel();
    moving.current.delete(row);
  }

  /** Take every transform this hook gave a row away at once. */
  function clear(row: HTMLElement) {
    stop(row);
    row.style.transform = "";
    row.style.scale = "";
    row.style.zIndex = "";
    row.style.willChange = "";
    row.removeAttribute("data-lifted");
  }

  /**
   * Move a row to `y` (relative to its place in the layout), from wherever it
   * is shown right now. `done` runs when it has arrived.
   */
  function slide(row: HTMLElement, y: number, done?: () => void) {
    const from = shownY(row);
    stop(row);
    row.style.transform = y ? `translateY(${y}px)` : "";
    if (prefersReducedMotion() || Math.abs(from - y) < 0.5) {
      done?.();
      return;
    }
    const animation = row.animate(
      [{ transform: `translateY(${from}px)` }, { transform: `translateY(${y}px)` }],
      { duration: SLIDE_MS, easing: EASE_OUT_SOFT },
    );
    moving.current.set(row, animation);
    void animation.finished
      .then(() => {
        if (moving.current.get(row) !== animation) return;
        moving.current.delete(row);
        done?.();
      })
      .catch(() => {});
  }

  /**
   * Up or down by the lifted card's scale. The individual `scale` property,
   * so it runs beside the transform that follows the pointer.
   */
  function lifted(card: HTMLElement, up: boolean) {
    if (up) card.setAttribute("data-lifted", "");
    else card.removeAttribute("data-lifted");
    const to = up ? String(LIFT_SCALE) : "1";
    const from = getComputedStyle(card).scale;
    card.style.scale = up ? to : "";
    if (prefersReducedMotion()) return;
    card.animate([{ scale: from === "none" ? "1" : from }, { scale: to }], {
      duration: SLIDE_MS,
      easing: EASE_OUT_SOFT,
    });
  }

  /**
   * The rows have just been laid out in their new order, still wearing the
   * transforms that showed them there before — measured from their old
   * places. Swap those for the new layout so that nothing moves; a row still
   * on its way finishes the trip from where it is. `offsetTop` rather than the
   * viewport: it is layout alone, untouched by transforms and by scrolling.
   */
  function settle(rows: HTMLElement[], places: Map<string, number>) {
    const shown = rows.map((row) => {
      const place = places.get(row.getAttribute(KEY) ?? "");
      return place === undefined ? null : place + shownY(row);
    });
    for (const row of rows) {
      stop(row);
      row.style.transform = "";
      row.style.willChange = "";
    }
    rows.forEach((row, index) => {
      const was = shown[index];
      const dy = was === null ? 0 : was - row.offsetTop;
      const done = () => (row.style.zIndex = "");
      if (prefersReducedMotion() || Math.abs(dy) < 0.5) {
        done();
        return;
      }
      const animation = row.animate(
        [{ transform: `translateY(${dy}px)` }, { transform: "none" }],
        { duration: SLIDE_MS, easing: EASE_OUT_SOFT },
      );
      moving.current.set(row, animation);
      void animation.finished
        .then(() => {
          if (moving.current.get(row) !== animation) return;
          moving.current.delete(row);
          done();
        })
        .catch(() => {});
    });
  }

  /** Everything back to where it stood, nothing written. */
  function goHome(rows: HTMLElement[], card: HTMLElement) {
    hideSlot();
    for (const row of rows) {
      if (row !== card) slide(row, 0);
    }
    lifted(card, false);
    slide(card, 0, () => {
      card.style.zIndex = "";
      card.style.willChange = "";
    });
  }

  function slotMark(): HTMLElement | null {
    return container.current?.querySelector<HTMLElement>(`[${SLOT}]`) ?? null;
  }

  function hideSlot() {
    const mark = slotMark();
    if (!mark) return;
    stop(mark);
    mark.hidden = true;
    mark.style.transform = "";
  }

  /** Measure the list and bring the card up. False when there is nothing to reorder. */
  function begin(s: Session): boolean {
    const root = container.current;
    const rows = rowsOf(root);
    const from = rows.findIndex((row) => row.getAttribute(KEY) === s.id);
    if (!root || from < 0 || rows.length < 2) return false;

    // A move still travelling (the arrow keys, say) ends where it was going:
    // measuring a row halfway would put every gap in the wrong place.
    for (const row of rows) {
      for (const animation of row.getAnimations()) animation.finish();
      clear(row);
    }
    const rects = rows.map((row) => row.getBoundingClientRect());
    const tops = rects.map((r) => r.top);
    const heights = rects.map((r) => r.height);
    const gap = rects[1].top - rects[0].bottom;
    const last = rows.length - 1;
    const scroller = scrollParent(root);
    const area = scroller?.getBoundingClientRect();
    const card = rows[from];

    s.lift = {
      rows,
      card,
      from,
      tops,
      heights,
      mids: rects.filter((_, i) => i !== from).map((r) => r.top + r.height / 2),
      step: heights[from] + gap,
      minDy: tops[0] - tops[from],
      maxDy: tops[last] + heights[last] - heights[from] - tops[from],
      slot: from,
      scroller,
      scroll0: scroller?.scrollTop ?? 0,
      // The scroll area's top padding is the room the floating navigation
      // covers; what shows of the page starts below it.
      edgeTop: area && scroller ? area.top + parseFloat(getComputedStyle(scroller).paddingTop) : 0,
      edgeBottom: area?.bottom ?? 0,
      frame: 0,
    };
    const mark = slotMark();
    if (mark) {
      // Where the card stands now, under it; it moves to the slot from here.
      mark.style.top = `${card.offsetTop}px`;
      mark.style.height = `${heights[from]}px`;
      mark.hidden = false;
    }
    card.style.zIndex = "10";
    card.style.willChange = "transform";
    lifted(card, true);
    s.userSelect = document.body.style.userSelect;
    document.body.style.userSelect = "none";
    if (scroller) {
      scroller.addEventListener("scroll", s.onScroll, { passive: true });
      const tick = () => {
        const lift = s.lift;
        if (!lift || s.done || session.current !== s) return;
        const speed = scrollStep(s.clientY, lift.edgeTop, lift.edgeBottom);
        if (speed !== 0) {
          const before = scroller.scrollTop;
          scroller.scrollTop += speed;
          if (scroller.scrollTop !== before) follow(s);
        }
        lift.frame = requestAnimationFrame(tick);
      };
      s.lift.frame = requestAnimationFrame(tick);
    }
    return true;
  }

  /** The card to the pointer, and the others out of its way. */
  function follow(s: Session) {
    const lift = s.lift;
    if (!lift || s.done) return;
    const scrolled = lift.scroller ? lift.scroller.scrollTop - lift.scroll0 : 0;
    const wanted = s.clientY - s.startY + scrolled;
    const dy = Math.min(lift.maxDy, Math.max(lift.minDy, wanted));
    lift.card.style.transform = `translateY(${dy}px)`;
    // From where the pointer wants the card, not where it is held: stopped at
    // the end of the list, its middle sits exactly on the last row's middle
    // and would never pass it.
    const slot = insertIndex(lift.mids, lift.tops[lift.from] + lift.heights[lift.from] / 2 + wanted);
    if (slot === lift.slot) return;
    lift.slot = slot;
    lift.rows.forEach((row, index) => {
      if (index !== lift.from) slide(row, shiftOf(index, lift.from, slot) * lift.step);
    });
    const mark = slotMark();
    if (mark) slide(mark, landingShift(lift.tops, lift.heights, lift.from, slot));
  }

  /** Put the card down in its slot and write the order, if it changed. */
  function drop(s: Session) {
    const lift = s.lift!;
    const order = lift.rows.map((row) => row.getAttribute(KEY) ?? "");
    const next = moveTo(order, lift.from, lift.slot);
    if (!next) {
      goHome(lift.rows, lift.card);
      return;
    }
    hideSlot();
    lifted(lift.card, false);
    // To exactly where the layout will put it; the others already stand there.
    slide(lift.card, landingShift(lift.tops, lift.heights, lift.from, lift.slot));
    const rows = lift.rows;
    pending.current = {
      places: new Map(rows.map((row) => [row.getAttribute(KEY) ?? "", row.offsetTop])),
      // A write that never comes back (it failed) must not leave the rows
      // standing in an order that is not the real one.
      timer: window.setTimeout(() => {
        pending.current = null;
        hold.current = false;
        goHome(rows, lift.card);
      }, WRITE_TIMEOUT_MS),
    };
    hold.current = true;
    move.current(next, s.id);
  }

  /** The press is over: let go of everything it held on to. */
  function finish(s: Session) {
    if (s.lift) {
      cancelAnimationFrame(s.lift.frame);
      s.lift.scroller?.removeEventListener("scroll", s.onScroll);
      document.body.style.userSelect = s.userSelect;
    }
    window.removeEventListener("keydown", s.onKey, true);
    s.done = true;
    if (session.current === s) session.current = null;
  }

  function grip(id: string): GripProps {
    const mine = (event: ReactPointerEvent<HTMLElement>) => {
      const s = session.current;
      return s && s.pointerId === event.pointerId ? s : null;
    };
    return {
      style: { touchAction: "none" },
      onPointerDown: (event) => {
        // The primary button only, one press at a time, and not while the last
        // drop is still on its way.
        if (event.button !== 0 || session.current || pending.current) return;
        event.currentTarget.setPointerCapture(event.pointerId);
        const s: Session = {
          pointerId: event.pointerId,
          id,
          startX: event.clientX,
          startY: event.clientY,
          clientY: event.clientY,
          lift: null,
          done: false,
          userSelect: "",
          onKey: (key) => {
            if (key.key !== "Escape" || !s.lift || s.done) return;
            // Ours alone: it must not also close whatever else listens for it.
            key.preventDefault();
            key.stopPropagation();
            goHome(s.lift.rows, s.lift.card);
            // The button is still down: ignore the rest of the press, but keep
            // it, so a second press cannot start before this one is up.
            cancelAnimationFrame(s.lift.frame);
            s.done = true;
          },
          onScroll: () => follow(s),
        };
        session.current = s;
        window.addEventListener("keydown", s.onKey, true);
      },
      onPointerMove: (event) => {
        const s = mine(event);
        if (!s || s.done) return;
        s.clientY = event.clientY;
        if (!s.lift) {
          const far = Math.hypot(event.clientX - s.startX, event.clientY - s.startY);
          if (far < THRESHOLD_PX) return;
          if (!begin(s)) {
            s.done = true;
            return;
          }
        }
        follow(s);
      },
      onPointerUp: (event) => {
        const s = mine(event);
        if (!s) return;
        if (s.lift && !s.done) drop(s);
        finish(s);
      },
      onPointerCancel: (event) => {
        const s = mine(event);
        if (!s) return;
        if (s.lift && !s.done) goHome(s.lift.rows, s.lift.card);
        finish(s);
      },
      // Capture can go without a pointerup — the window losing focus mid-drag.
      // After a pointerup the session is already gone and this does nothing.
      onLostPointerCapture: (event) => {
        const s = mine(event);
        if (!s) return;
        if (s.lift && !s.done) goHome(s.lift.rows, s.lift.card);
        finish(s);
      },
    };
  }

  return { grip };
}

function rowsOf(root: HTMLElement | null): HTMLElement[] {
  return root ? [...root.querySelectorAll<HTMLElement>(`[${KEY}]`)] : [];
}

/** How far a row is shown from its place in the layout, mid-animation included. */
function shownY(row: HTMLElement): number {
  const value = getComputedStyle(row).transform;
  if (!value || value === "none") return 0;
  return new DOMMatrixReadOnly(value).m42;
}

/** The nearest ancestor that scrolls vertically. */
function scrollParent(node: HTMLElement): HTMLElement | null {
  for (let at = node.parentElement; at; at = at.parentElement) {
    const { overflowY } = getComputedStyle(at);
    if (overflowY === "auto" || overflowY === "scroll") return at;
  }
  return null;
}
