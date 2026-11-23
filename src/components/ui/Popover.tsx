import { useEffect, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";
import { cn } from "@/lib/cn";
import { placeMenu } from "@/lib/menuPlace";
import { FLOATING } from "./Menu";

/** Distance to the window edge, as for every menu. */
const EDGE = 8;
/** Space between the button and the surface. */
const GAP = 6;

/**
 * A floating surface under its button, like the list behind "Add source": the
 * menus' fill, placed like every menu, its right edge on the button's. Escape,
 * a click outside, scrolling the page or a resize closes it. Not the shared
 * `Menu`, which takes no focus — these hold a field to type in, and the focus
 * moves inside on open.
 */
export function Popover({
  anchor,
  label,
  onClose,
  className,
  children,
}: {
  /** The button it hangs from — a click on it is not a click outside. */
  anchor: RefObject<HTMLElement | null>;
  label: string;
  onClose: () => void;
  className?: string;
  children: ReactNode;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [place, setPlace] = useState<{ left: number; top: number; maxHeight: number } | null>(null);

  // Measured at its natural size; an answer line appearing places it again.
  useLayoutEffect(() => {
    const el = box.current;
    const button = anchor.current;
    if (!el || !button) return;
    const measure = () => {
      const kept = el.style.maxHeight;
      el.style.maxHeight = "none";
      const p = placeMenu({
        anchor: button.getBoundingClientRect(),
        size: { width: el.offsetWidth, height: el.offsetHeight },
        viewport: { width: window.innerWidth, height: window.innerHeight },
        margin: EDGE,
        gap: GAP,
        alignRight: true,
        slide: false,
      });
      const height = el.offsetHeight;
      el.style.maxHeight = kept;
      setPlace((old) =>
        old && old.left === p.x && old.top === p.y && old.maxHeight === Math.min(height, p.maxHeight)
          ? old
          : { left: p.x, top: p.y, maxHeight: Math.min(height, p.maxHeight) },
      );
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el.firstElementChild ?? el);
    return () => observer.disconnect();
  }, [anchor]);

  // Focus moves in on open, so the keyboard goes on where the eyes do: to what
  // the content marks with `data-autofocus`, else its first field or button.
  // Content that took the focus itself (a field it selects) keeps it.
  useEffect(() => {
    const el = box.current;
    if (!el || el.contains(document.activeElement)) return;
    const target =
      el.querySelector<HTMLElement>("[data-autofocus]") ??
      el.querySelector<HTMLElement>("input:not([disabled]), button:not([disabled]), [tabindex]:not([tabindex='-1'])");
    target?.focus({ preventScroll: true });
  }, []);

  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const onDown = (event: PointerEvent) => {
      const target = event.target as Node;
      if (box.current?.contains(target) || anchor.current?.contains(target)) return;
      close.current();
    };
    const onScroll = (event: Event) => {
      if (event.target instanceof Node && box.current?.contains(event.target)) return;
      close.current();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      (anchor.current?.querySelector<HTMLElement>("button") ?? anchor.current)?.focus();
      close.current();
    };
    const onAway = () => close.current();
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("resize", onAway);
    return () => {
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("resize", onAway);
    };
  }, [anchor]);

  return createPortal(
    <div
      ref={box}
      role="dialog"
      aria-label={label}
      style={{
        left: place?.left ?? 0,
        top: place?.top ?? 0,
        maxHeight: place?.maxHeight,
        background: FLOATING,
        backdropFilter: "blur(20px)",
      }}
      className={cn(
        "fixed z-[70] flex flex-col overflow-y-auto overscroll-contain rounded-[16px] border border-white/10",
        "shadow-[0_8px_32px_rgba(0,0,0,0.4)] origin-top-right",
        "transition-[opacity,transform] duration-100 ease-[var(--ease-out-soft)]",
        place ? "scale-100 opacity-100" : "scale-[0.97] opacity-0",
        className,
      )}
    >
      <div>{children}</div>
    </div>,
    document.fullscreenElement ?? document.body,
  );
}
