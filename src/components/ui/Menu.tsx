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

/** Ein Eintrag im Menü. Trenner tragen nichts als sich selbst. */
export type MenuEntry =
  | {
      kind: "item";
      label: string;
      icon?: ReactNode;
      /** Rechts angezeigtes Tastenkürzel, etwa „Strg+C". */
      shortcut?: string;
      danger?: boolean;
      disabled?: boolean;
      onSelect: () => void;
    }
  | { kind: "separator" };

/**
 * Was zum Öffnen reicht: Position und die Möglichkeit, das Ereignis
 * abzufangen. Passt sowohl auf React-Ereignisse als auch auf die des
 * Dokuments — das Menü in den Textfeldern kommt von dort.
 */
export interface MenuTrigger {
  clientX: number;
  clientY: number;
  preventDefault: () => void;
  stopPropagation: () => void;
}

interface MenuApi {
  /**
   * Menü an der Position des Ereignisses aufmachen. Fängt das Ereignis ab —
   * das eingebaute Menü des WebViews bleibt damit zu.
   */
  open: (trigger: MenuTrigger, entries: MenuEntry[]) => void;
  close: () => void;
}

const Context = createContext<MenuApi | null>(null);

/** Zum Öffnen eines Menüs — überall unter dem {@link MenuProvider}. */
export function useMenu(): MenuApi {
  const api = useContext(Context);
  if (!api) throw new Error("useMenu braucht einen MenuProvider");
  return api;
}

interface Open {
  x: number;
  y: number;
  entries: MenuEntry[];
  /** Im Vollbild ist alles außerhalb des Vollbild-Elements unsichtbar. */
  host: Element;
}

/**
 * Hält genau ein Menü. Mehr braucht es nicht: Ein zweiter Rechtsklick
 * ersetzt, was gerade offen ist.
 */
export function MenuProvider({ children }: { children: ReactNode }) {
  const [open, setOpen] = useState<Open | null>(null);

  const close = useCallback(() => setOpen(null), []);
  // `setOpen` ist stabil, also ist es die Schnittstelle auch — sonst hinge an
  // jedem Render ein neues Objekt und jeder Aufrufer renderte mit.
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

/** Abstand zum Fensterrand, damit das Menü nicht klebt. */
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
  /** Womit die Tastatur gerade arbeitet. -1 heißt: nichts hervorgehoben. */
  const [active, setActive] = useState(-1);

  // Kippen, statt aus dem Fenster zu laufen. Erst nach dem Einhängen — vorher
  // ist die Größe des Menüs nicht bekannt.
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
    // Alles, was den Blick woanders hinlenkt, schließt das Menü.
    const onScroll = () => onClose();
    // Ein Klick **im** Menü darf es nicht schließen, bevor der Eintrag
    // ausgelöst hat — sonst verschwände der Knopf unter dem Mauszeiger und
    // `click` käme nie an.
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
    // In der Erfassungsphase, sonst entgeht uns das Scrollen in der Galerie.
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
      // Das Menü nimmt bewusst keinen Fokus: Sonst verlöre das Textfeld
      // darunter seine Auswahl — und der Editor speicherte beim Blur.
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
            {/* Feste Spalte für das Symbol, damit die Beschriftungen auch
                ohne eines auf einer Linie stehen. */}
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
