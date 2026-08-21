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

interface MenuApi {
  /**
   * Open the menu at the event's position. Swallows the event — that keeps the
   * WebView's built-in menu shut.
   */
  open: (trigger: MenuTrigger, entries: MenuEntry[]) => void;
  close: () => void;
}

const Context = createContext<MenuApi | null>(null);

/** For opening a menu — anywhere below the {@link MenuProvider}. */
export function useMenu(): MenuApi {
  const api = useContext(Context);
  if (!api) throw new Error("useMenu needs a MenuProvider");
  return api;
}

interface Open {
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
      open: (trigger, entries) => {
        trigger.preventDefault();
        trigger.stopPropagation();
        if (entries.length === 0) return;
        setOpen({
          x: trigger.clientX,
          y: trigger.clientY,
          entries,
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

/** Distance to the window edge so the menu does not stick to it. */
const EDGE = 8;

function Surface({
  x,
  y,
  entries,
  onClose,
}: Open & { onClose: () => void }) {
  const box = useRef<HTMLDivElement>(null);
  const [at, setAt] = useState({ x, y });
  const [shown, setShown] = useState(false);
  /** What the keyboard is working on. -1 means nothing is highlighted. */
  const [active, setActive] = useState(-1);

  // Flip instead of running out of the window. Only after mounting — before
  // that the menu's size is not known.
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    setAt({
      x: Math.max(EDGE, Math.min(x, window.innerWidth - width - EDGE)),
      y: Math.max(EDGE, Math.min(y, window.innerHeight - height - EDGE)),
    });
    setShown(true);
  }, [x, y]);

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
    const onScroll = () => onClose();
    // A click **inside** the menu must not close it before the entry has fired —
    // otherwise the button would vanish from under the cursor and `click` would
    // never arrive.
    const onDown = (event: MouseEvent) => {
      if (box.current?.contains(event.target as Node)) return;
      onClose();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        if (selectable.length === 0) return;
        const step = event.key === "ArrowDown" ? 1 : -1;
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

    window.addEventListener("mousedown", onDown);
    window.addEventListener("blur", onClose);
    window.addEventListener("resize", onClose);
    window.addEventListener("keydown", onKey, true);
    // In the capture phase, otherwise we miss scrolling in the gallery.
    window.addEventListener("scroll", onScroll, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
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
      style={{ left: at.x, top: at.y }}
      className={cn(
        "fixed z-[60] min-w-[224px] py-1.5",
        "rounded-inner border border-line bg-elevated/95 backdrop-blur-xl",
        "shadow-[0_16px_48px_rgba(0,0,0,0.55)]",
        "origin-top-left transition-[opacity,transform] duration-100 ease-[var(--ease-out-soft)]",
        shown ? "scale-100 opacity-100" : "scale-[0.97] opacity-0",
      )}
    >
      {entries.map((entry, index) =>
        entry.kind === "separator" ? (
          <div key={index} className="my-1.5 border-t border-line" />
        ) : (
          <button
            key={index}
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
  );
}
