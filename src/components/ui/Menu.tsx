import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

import { cn } from "@/lib/cn";
import { placeMenu, type Anchor } from "@/lib/menuPlace";
import { menuOwns } from "@/lib/viewer";

/** One entry in the menu. Separators carry nothing but themselves. */
export type MenuEntry =
  | {
      kind: "item";
      label: string;
      icon?: ReactNode;
      /** Shortcut shown on the right, "Ctrl+C" for instance. */
      shortcut?: string;
      danger?: boolean;
      disabled?: boolean;
      onSelect: () => void;
    }
  | { kind: "separator" };

/**
 * What is enough to open: a position and the ability to swallow the event. Fits
 * both React events and the document's own — the text field menu comes from
 * there.
 */
export interface MenuTrigger {
  clientX: number;
  clientY: number;
  preventDefault: () => void;
  stopPropagation: () => void;
}

/** How a menu looks and sits, beyond its entries. */
export interface MenuOptions {
  /** The surface of the nav bar pill instead of the context menu's. */
  glass?: boolean;
  /** The trigger's x is the menu's right edge, not its left. */
  alignRight?: boolean;
  /**
   * The button the menu hangs from. With it the menu can flip above the button
   * when there is no room below; without it the trigger's point is the anchor.
   */
  anchor?: Anchor;
  /** Space between the anchor and the menu. */
  gap?: number;
}

interface MenuApi {
  /**
   * Open the menu at the event's position. Swallows the event — that keeps the
   * WebView's built-in menu shut.
   */
  open: (trigger: MenuTrigger, entries: MenuEntry[], options?: MenuOptions) => void;
  close: () => void;
}

const Context = createContext<MenuApi | null>(null);

/** For opening a menu — anywhere below the {@link MenuProvider}. */
export function useMenu(): MenuApi {
  const api = useContext(Context);
  if (!api) throw new Error("useMenu needs a MenuProvider");
  return api;
}

interface Open extends MenuOptions {
  x: number;
  y: number;
  entries: MenuEntry[];
  /** In fullscreen everything outside the fullscreen element is invisible. */
  host: Element;
}

/**
 * Holds exactly one menu. No more is needed: a second right-click replaces
 * whatever is open.
 */
export function MenuProvider({ children }: { children: ReactNode }) {
  const [open, setOpen] = useState<Open | null>(null);

  const close = useCallback(() => setOpen(null), []);
  // `setOpen` is stable, so the interface is too — otherwise every render would
  // hang a new object off it and every caller would re-render along.
  const api = useMemo<MenuApi>(
    () => ({
      open: (trigger, entries, options) => {
        trigger.preventDefault();
        trigger.stopPropagation();
        if (entries.length === 0) return;
        setOpen({
          x: trigger.clientX,
          y: trigger.clientY,
          entries,
          ...options,
          host: document.fullscreenElement ?? document.body,
        });
      },
      close: () => setOpen(null),
    }),
    [],
  );

  return (
    <Context.Provider value={api}>
      {children}
      {open && createPortal(<Surface {...open} onClose={close} />, open.host)}
    </Context.Provider>
  );
}

/** The floating surfaces' fill: the elevated colour, nearly opaque, so nothing behind reads through. */
export const FLOATING = "color-mix(in srgb, var(--color-elevated) 96%, transparent)";

/** Distance to the window edge so the menu does not stick to it. */
const EDGE = 8;

function Surface({
  x,
  y,
  entries,
  glass,
  alignRight,
  anchor,
  gap,
  onClose,
}: Open & { onClose: () => void }) {
  const box = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const [at, setAt] = useState({ x, y });
  /** Undefined until measured: the natural height is read without a limit. */
  const [maxHeight, setMaxHeight] = useState<number>();
  /** Set by the arrow keys, so only they scroll the list. */
  const byKey = useRef(false);
  const [shown, setShown] = useState(false);
  /** What the keyboard is working on. -1 means nothing is highlighted. */
  const [active, setActive] = useState(-1);

  // Flip or shrink instead of running out of the window. Only after mounting —
  // before that the menu's size is not known.
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    // The layout size: the entry animation starts at 97 % scale, which
    // getBoundingClientRect would report.
    const { offsetWidth: width, offsetHeight: height } = el;
    const place = placeMenu({
      anchor: anchor ?? { left: x, top: y, right: x, bottom: y },
      size: { width, height },
      viewport: { width: window.innerWidth, height: window.innerHeight },
      margin: EDGE,
      gap: anchor ? (gap ?? 0) : 0,
      alignRight,
    });
    setAt({ x: place.x, y: place.y });
    setMaxHeight(place.maxHeight);
    setShown(true);
  }, [x, y, alignRight, anchor, gap]);

  // The keyboard walks the list: keep the highlighted entry in view.
  useEffect(() => {
    if (!byKey.current || active < 0) return;
    byKey.current = false;
    list.current
      ?.querySelector<HTMLElement>(`[data-index="${active}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const selectable = entries
    .map((entry, index) => ({ entry, index }))
    .filter(({ entry }) => entry.kind === "item" && !entry.disabled)
    .map(({ index }) => index);

  const run = (entry: MenuEntry) => {
    if (entry.kind !== "item" || entry.disabled) return;
    onClose();
    entry.onSelect();
  };

  useEffect(() => {
    // Anything that draws the eye elsewhere closes the menu.
    // The menu's own list scrolling is not "looking elsewhere".
    const onScroll = (event: Event) => {
      if (event.target instanceof Node && box.current?.contains(event.target)) return;
      onClose();
    };
    // A click **inside** the menu must not close it before the entry has fired —
    // otherwise the button would vanish from under the cursor and `click` would
    // never arrive.
    // `pointerdown` in the capture phase, not `mousedown`: a stage that pans
    // calls preventDefault and stopPropagation on the pointer, and the mouse
    // event then never comes.
    const onDown = (event: PointerEvent) => {
      if (box.current?.contains(event.target as Node)) return;
      onClose();
    };
    const onKey = (event: KeyboardEvent) => {
      // The menu owns these keys: the viewer underneath must not also close
      // or change its volume. Not stopImmediatePropagation, the lockdown
      // listener on window must still run.
      if (menuOwns(event.key, active)) event.stopPropagation();
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        if (selectable.length === 0) return;
        const step = event.key === "ArrowDown" ? 1 : -1;
        byKey.current = true;
        setActive((current) => {
          const at = selectable.indexOf(current);
          const next = at === -1 ? (step === 1 ? 0 : selectable.length - 1) : at + step;
          return selectable[(next + selectable.length) % selectable.length];
        });
        return;
      }
      if (event.key === "Enter" && active >= 0) {
        event.preventDefault();
        run(entries[active]);
      }
    };

    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("blur", onClose);
    window.addEventListener("resize", onClose);
    window.addEventListener("keydown", onKey, true);
    // In the capture phase, otherwise we miss scrolling in the gallery.
    window.addEventListener("scroll", onScroll, true);
    return () => {
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("blur", onClose);
      window.removeEventListener("resize", onClose);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("scroll", onScroll, true);
    };
  }, [active, entries, onClose, selectable, run]);

  return (
    <div
      ref={box}
      role="menu"
      // The menu deliberately takes no focus: the text field underneath would
      // otherwise lose its selection — and the editor would save on blur.
      onMouseDown={(event) => event.preventDefault()}
      onContextMenu={(event) => event.preventDefault()}
      style={{
        left: at.x,
        top: at.y,
        maxHeight,
        ...(glass && { background: FLOATING, backdropFilter: "blur(20px)" }),
      }}
      className={cn(
        "fixed z-[75] flex min-w-[224px] flex-col py-1.5",
        glass
          ? "rounded-inner border border-white/10 shadow-[0_8px_32px_rgba(0,0,0,0.4)]"
          : "rounded-inner border border-line bg-elevated/[0.96] backdrop-blur-xl shadow-[0_16px_48px_rgba(0,0,0,0.55)]",
        "origin-top-left transition-[opacity,transform] duration-100 ease-[var(--ease-out-soft)]",
        shown ? "scale-100 opacity-100" : "scale-[0.97] opacity-0",
      )}
    >
      <div ref={list} className="min-h-0 flex-1 overflow-y-auto overscroll-contain">
        {entries.map((entry, index) =>
          entry.kind === "separator" ? (
            <div key={index} className="my-1.5 border-t border-line" />
          ) : (
            <button
              key={index}
              data-index={index}
              role="menuitem"
              disabled={entry.disabled}
              onMouseEnter={() => setActive(index)}
              onMouseLeave={() => setActive(-1)}
              onClick={() => run(entry)}
              className={cn(
                "mx-1.5 flex h-9 w-[calc(100%-0.75rem)] items-center gap-2.5 rounded-inner px-2.5",
                "text-left text-[13px] font-medium transition-colors duration-100",
                "disabled:pointer-events-none disabled:opacity-35",
                entry.danger ? "text-live" : "text-ink",
                active === index && (entry.danger ? "bg-live/15" : "bg-hover"),
              )}
            >
              {/* A fixed column for the icon, so labels line up even without
                  one. */}
              <span className="grid h-4 w-4 shrink-0 place-items-center text-ink-muted">
                {entry.icon}
              </span>
              <span className="min-w-0 flex-1 truncate">{entry.label}</span>
              {entry.shortcut && (
                <span className="shrink-0 text-xs text-ink-faint tabular-nums">
                  {entry.shortcut}
                </span>
              )}
            </button>
          ),
        )}
      </div>
    </div>
  );
}
