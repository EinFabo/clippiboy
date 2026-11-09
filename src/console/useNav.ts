import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "@/lib/ipc";
import { navigate, nearest, nextTab, rangeSteps, type Box, type Dir, type Stop } from "@/lib/spatialNav";

/**
 * The console steered with a controller.
 *
 * The core reads the pads while the console is open and sends each step as
 * `console-nav` (`consolenav.rs`): a direction, `activate`, `back`,
 * `prev`/`next` for the panels, `close`. Here a ring walks over what can be
 * pressed — `button`s, range inputs, anything marked `data-nav` — to the
 * nearest one in the direction pushed (`spatialNav.ts`), and `activate`
 * clicks it. Keyboard and mouse stay exactly as they are: the ring shows from
 * the first step on, and goes when the mouse moves.
 *
 * The page marks out the rest:
 * - `data-nav-region` — something that opens over or beside the rest (a
 *   panel, the player, the question before deleting). When one appears, the
 *   ring goes into it, and when it goes, back to what opened it;
 * - `data-nav-trap` — a region the ring stays inside while it stands;
 * - `data-nav-default` — where the ring lands in a region (or `data-nav-home`,
 *   the dock) when it comes in;
 * - `data-nav-tab` — the panel buttons the bumpers go round (not while a
 *   trap stands);
 * - `data-nav-keys` — needs the keyboard (rename): left out while the console
 *   stands without the focus, where typing would go to the game;
 * - `data-nav-skip` — never;
 * - `data-nav-rows` / `data-nav-row` — a group laid out in rows (the
 *   player): inside it the ring goes round each row and from row to row
 *   instead of by geometry, which left Play out of reach from the right;
 * - `data-nav-adjust` — `engage`: a control that takes left/right once
 *   activated (A), until A or B again or a step up or down — range inputs are
 *   this anyway; `direct`: takes left/right at once (the picture, which is a
 *   row of its own). Adjusting sends a `nav-adjust` event (detail ±1), a
 *   range input is moved itself;
 * - `data-nav-id` — a name that outlives the element, for finding the
 *   control that opened something after it was drawn anew.
 *
 * Text fields are left out: a controller cannot type into them.
 */

type Step = Dir | "activate" | "back" | "prev" | "next" | "close";

const RING = "data-nav-focus";
const EDITING = "data-nav-editing";
const CANDIDATES = 'button, input[type="range"], [data-nav]';
/** The mouse has to travel this far before it takes over from the ring — a
 *  game that keeps putting its hidden cursor back in the middle sends moves
 *  on the spot. */
const MOUSE_PX = 6;

function boxOf(element: Element): Box {
  const r = element.getBoundingClientRect();
  return { left: r.left, top: r.top, width: r.width, height: r.height };
}

/** On screen, not on its way out, and taking clicks. */
function standing(element: Element): boolean {
  if (element.closest('[data-leaving="true"]')) return false;
  const r = element.getBoundingClientRect();
  if (r.width <= 0 || r.height <= 0) return false;
  // Not `opacityProperty`: a panel coming in starts at opacity 0.
  if (element.checkVisibility && !element.checkVisibility({ visibilityProperty: true })) {
    return false;
  }
  return getComputedStyle(element).pointerEvents !== "none";
}

/** Where the ring may stand. A disabled button too — the dock's Clip while
 *  it saves: the ring stays on it rather than jumping to a neighbour. */
function holdable(element: Element | null): element is HTMLElement {
  if (!(element instanceof HTMLElement) || !element.isConnected) return false;
  if (element.closest("[data-nav-skip]")) return false;
  if (element.hasAttribute("data-nav-keys") && !document.hasFocus()) return false;
  return standing(element);
}

/** Where the ring may go. */
function usable(element: Element): element is HTMLElement {
  return holdable(element) && !(element as HTMLButtonElement).disabled;
}

function isRange(element: Element | null): element is HTMLInputElement {
  return element instanceof HTMLInputElement && element.type === "range";
}

/** Takes left/right once activated. */
function engages(element: Element | null): boolean {
  return isRange(element) || element?.getAttribute("data-nav-adjust") === "engage";
}

/** Left or right on it, once taken. */
function adjust(element: HTMLElement, by: 1 | -1) {
  if (isRange(element)) nudge(element, by);
  else element.dispatchEvent(new CustomEvent("nav-adjust", { detail: by, bubbles: true }));
}

/** The controls as {@link navigate} wants them: a group and row for those in
 *  a `data-nav-rows` group. */
function stopsOf(pool: HTMLElement[]): Stop[] {
  const groups = [...document.querySelectorAll("[data-nav-rows]")];
  return pool.map((element) => {
    const row = element.closest("[data-nav-row]");
    const group = row?.closest("[data-nav-rows]");
    if (!row || !group) return { box: boxOf(element) };
    return {
      box: boxOf(element),
      group: groups.indexOf(group),
      row: [...group.querySelectorAll("[data-nav-row]")].indexOf(row),
    };
  });
}

function candidatesIn(scope: ParentNode): HTMLElement[] {
  return [...scope.querySelectorAll(CANDIDATES)].filter(usable);
}

function regions(): Element[] {
  return [...document.querySelectorAll("[data-nav-region]")].filter(standing);
}

/** The innermost trap standing, if any. */
function trap(): Element | null {
  const traps = regions().filter((r) => r.hasAttribute("data-nav-trap"));
  return traps[traps.length - 1] ?? null;
}

function landingIn(container: ParentNode): HTMLElement | null {
  const marked = container.querySelector("[data-nav-default]");
  if (marked && usable(marked)) return marked;
  return candidatesIn(container)[0] ?? null;
}

/** Scroll the list a control sits in until it is in view — the panel, say.
 *  Not `scrollIntoView`: that scrolls the page itself too when a control
 *  pokes out of the window, and the page is the screen and must not move. */
function reveal(element: HTMLElement) {
  for (let box = element.parentElement; box && box !== document.body; box = box.parentElement) {
    if (box.scrollHeight <= box.clientHeight) continue;
    const overflow = getComputedStyle(box).overflowY;
    if (overflow !== "auto" && overflow !== "scroll") continue;
    const outer = box.getBoundingClientRect();
    const inner = element.getBoundingClientRect();
    // Room for the ring around it.
    const margin = 8;
    // The rectangles are on the screen, `scrollTop` in the box's own pixels —
    // they differ by the console's size setting (`zoom` on `.cb-ui`).
    const k = outer.height > 0 ? box.offsetHeight / outer.height : 1;
    if (inner.top - margin < outer.top) box.scrollTop -= (outer.top - inner.top + margin) * k;
    else if (inner.bottom + margin > outer.bottom) box.scrollTop += (inner.bottom - outer.bottom + margin) * k;
    return;
  }
}

/** Move a range input's knob the way React hears it — `input` for the value,
 *  `change` for whoever waits for the knob to come to rest. */
function nudge(input: HTMLInputElement, by: 1 | -1) {
  const steps = rangeSteps(Number(input.min || 0), Number(input.max || 100), Number(input.step || 1));
  const before = input.value;
  if (by > 0) input.stepUp(steps);
  else input.stepDown(steps);
  if (input.value === before) return;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  input.dispatchEvent(new Event("change", { bubbles: true }));
}

export function useConsoleNav({
  live,
  close,
  opened,
}: {
  /** The console is up and not on its way out — steps count. */
  live: () => boolean;
  /** Close the console at once (Start). */
  close: () => void;
  /** Counts openings: each one starts without a ring. */
  opened: number;
}) {
  const liveRef = useRef(live);
  liveRef.current = live;
  const closeRef = useRef(close);
  closeRef.current = close;

  const state = useRef({
    current: null as HTMLElement | null,
    shown: false,
    /** The control under the ring has taken left/right (`data-nav-adjust`). */
    editing: false,
    /** Where the ring last stood, for when that control is gone. */
    lastBox: null as Box | null,
    /** The regions gone into, each with what the ring stood on before — the
     *  element, and its `data-nav-id` for when it has been drawn anew. */
    stack: [] as { region: Element; opener: HTMLElement | null; openerId: string | null }[],
    seen: new WeakSet<Element>(),
    /** What a region coming up next should lead back to, when not the
     *  control the ring stands on — the panel button a bumper pressed. */
    opener: null as HTMLElement | null,
    /** Where the mouse was when the ring came up. */
    pointer: null as { x: number; y: number } | null,
  });

  useEffect(() => {
    const s = state.current;
    let disposed = false;

    const paint = () => {
      document.querySelectorAll(`[${RING}]`).forEach((element) => {
        if (element !== s.current || !s.shown) {
          element.removeAttribute(RING);
          element.removeAttribute(EDITING);
        }
      });
      if (s.shown && s.current) {
        s.current.setAttribute(RING, "");
        if (s.editing) s.current.setAttribute(EDITING, "");
        else s.current.removeAttribute(EDITING);
        reveal(s.current);
      }
    };

    const put = (element: HTMLElement | null) => {
      if (element !== s.current) s.editing = false;
      s.current = element;
      if (element) s.lastBox = boxOf(element);
      paint();
    };

    const valid = (element: HTMLElement | null, scope: Element | null): element is HTMLElement =>
      holdable(element) && (!scope || scope.contains(element));

    /** What opened a region, as it stands now — the same element, or the one
     *  drawn in its place under the same `data-nav-id`. */
    const opener = (
      entry: { opener: HTMLElement | null; openerId: string | null },
      scope: Element | null,
    ): HTMLElement | null => {
      if (valid(entry.opener, scope)) return entry.opener;
      if (!entry.openerId) return null;
      const again = document.querySelector(`[data-nav-id="${CSS.escape(entry.openerId)}"]`);
      return again instanceof HTMLElement && valid(again, scope) ? again : null;
    };

    /** Where the ring goes with nothing else to go by. */
    const landing = (): HTMLElement | null => {
      const inTrap = trap();
      if (inTrap) return landingIn(inTrap);
      const open = regions();
      for (let i = open.length - 1; i >= 0; i -= 1) {
        const target = landingIn(open[i]);
        if (target) return target;
      }
      const home = document.querySelector("[data-nav-home]");
      return (home && landingIn(home)) ?? candidatesIn(document)[0] ?? null;
    };

    const standingRegion = (region: Element) => region.isConnected && standing(region);

    /** Bring the ring in line with the page as it now stands: into a region
     *  that just came up, out of one that went. */
    const settle = () => {
      const standingRegions = regions();
      const fresh = standingRegions.filter((r) => !s.seen.has(r));
      standingRegions.forEach((r) => s.seen.add(r));
      const pending = s.opener;
      s.opener = null;
      for (let i = fresh.length - 1; i >= 0; i -= 1) {
        const target = landingIn(fresh[i]);
        if (!target) continue;
        const from = pending ?? s.current;
        s.stack.push({ region: fresh[i], opener: from, openerId: from?.dataset.navId ?? null });
        put(target);
        return;
      }
      const scope = trap();
      if (valid(s.current, scope)) {
        s.lastBox = boxOf(s.current);
        // The ring walked out of the regions gone into (down into the dock,
        // say) and they closed behind it: they lead nowhere any more. Not
        // while it stands inside one of them — the player stands where the
        // clip list was, and closing it goes back past the list.
        const current = s.current;
        const inside = s.stack.some((e) => standingRegion(e.region) && e.region.contains(current));
        if (!inside) s.stack = s.stack.filter((e) => standingRegion(e.region));
        return;
      }
      // Out of every region that went, back to what opened the last of them.
      while (s.stack.length > 0) {
        const top = s.stack[s.stack.length - 1];
        if (standingRegion(top.region)) break;
        s.stack.pop();
        const back = opener(top, scope);
        if (back) {
          put(back);
          return;
        }
      }
      const pool = candidatesIn(scope ?? document);
      if (s.lastBox && pool.length > 0) {
        put(pool[nearest(s.lastBox, pool.map(boxOf))]);
        return;
      }
      put(landing());
    };

    /** Settle again once the page has drawn what a press changed. */
    const later = () => {
      requestAnimationFrame(() =>
        requestAnimationFrame(() => {
          if (!disposed && liveRef.current()) settle();
        }),
      );
    };

    const move = (dir: Dir) => {
      const from = s.current;
      if (!from) return;
      const sideways = dir === "left" || dir === "right";
      const by = dir === "right" ? 1 : -1;
      if (sideways && (s.editing || from.getAttribute("data-nav-adjust") === "direct")) {
        adjust(from, by);
        return;
      }
      // Up or down ends adjusting and goes on.
      s.editing = false;
      const pool = candidatesIn(trap() ?? document);
      if (!pool.includes(from)) pool.push(from);
      const at = navigate(stopsOf(pool), pool.indexOf(from), dir);
      if (at >= 0) put(pool[at]);
      else paint();
    };

    const tab = (by: 1 | -1) => {
      // The question before deleting, the send list: answered or backed out
      // of first.
      if (trap()) return;
      const tabs = candidatesIn(document).filter((element) => element.hasAttribute("data-nav-tab"));
      const open = tabs.findIndex((element) => element.getAttribute("aria-pressed") === "true");
      const target = tabs[nextTab(tabs.length, open, by)];
      if (!target) return;
      s.opener = target;
      put(target);
      target.click();
    };

    const step = (what: Step) => {
      if (!liveRef.current()) return;
      settle();
      const wasShown = s.shown;
      if (!wasShown) {
        s.shown = true;
        s.pointer = null;
        if (!s.current) put(landing());
        else paint();
      }
      switch (what) {
        case "up":
        case "down":
        case "left":
        case "right":
          // The first step only shows where the ring is.
          if (wasShown) move(what);
          return;
        case "activate":
          if (!wasShown || !s.current) return;
          if (engages(s.current)) {
            s.editing = !s.editing;
            paint();
            return;
          }
          if (!(s.current as HTMLButtonElement).disabled) s.current.click();
          break;
        case "back":
          if (s.editing) {
            s.editing = false;
            paint();
            return;
          }
          // Exactly what Escape does: one step out, the console at the end.
          window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
          break;
        case "prev":
          tab(-1);
          break;
        case "next":
          tab(1);
          break;
        case "close":
          closeRef.current();
          return;
      }
      later();
    };

    const onPointer = (event: PointerEvent) => {
      if (!s.shown) return;
      if (!s.pointer) {
        s.pointer = { x: event.clientX, y: event.clientY };
        return;
      }
      if (Math.hypot(event.clientX - s.pointer.x, event.clientY - s.pointer.y) < MOUSE_PX) return;
      s.shown = false;
      s.editing = false;
      paint();
    };
    window.addEventListener("pointermove", onPointer);

    const off = inTauri ? listen<Step>("console-nav", (e) => step(e.payload), { target: "console" }) : null;
    return () => {
      disposed = true;
      window.removeEventListener("pointermove", onPointer);
      void off?.then((fn) => fn());
    };
  }, []);

  // Every opening starts over: no ring until the first step.
  useEffect(() => {
    const s = state.current;
    s.current?.removeAttribute(RING);
    s.current?.removeAttribute(EDITING);
    s.current = null;
    s.shown = false;
    s.editing = false;
    s.lastBox = null;
    s.stack = [];
    s.seen = new WeakSet();
    s.opener = null;
    s.pointer = null;
  }, [opened]);
}
