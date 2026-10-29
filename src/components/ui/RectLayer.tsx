import { useRef, type PointerEvent as ReactPointerEvent, type RefObject } from "react";
import { cn } from "@/lib/cn";
import { usePictureBox } from "@/lib/pictureBox";

/** A rectangle in pixels of the picture — never of the preview. */
export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * A rectangle over the picture that moves and resizes by its corners.
 *
 * Everything here is reckoned in pixels of the picture, and only turned into
 * percentages when it is drawn. That way the rectangle survives a resized
 * window and means the same thing on a 4K still as on a 1080p one — and the
 * core gets it in the only unit it can use.
 *
 * `crop`: a press on the picture draws a new selection, everything outside it
 * goes dark, thirds are drawn. `box`: only the rectangle takes the pointer —
 * a click beside it reaches what lies below (the player's pause and
 * fullscreen). `onRelease` hears every let-go.
 */
export function RectLayer({
  stage,
  width,
  height,
  ratio,
  rect,
  onRect,
  onRelease,
  mode = "crop",
}: {
  stage: RefObject<HTMLDivElement | null>;
  width: number;
  height: number;
  ratio: number | null;
  rect: Rect | null;
  onRect: (rect: Rect) => void;
  onRelease?: (rect: Rect | null) => void;
  mode?: "crop" | "box";
}) {
  const crop = mode === "crop";
  const box = usePictureBox(stage, width, height);
  /** What the pointer is doing right now. `null` means: nothing. */
  const drag = useRef<
    | { kind: "draw"; anchorX: number; anchorY: number }
    | { kind: "move"; grabX: number; grabY: number }
    | null
  >(null);

  if (!box) return null;

  const clamp = (value: number, max: number) => Math.min(Math.max(value, 0), max);

  /** Pointer position in pixels of the picture. */
  const at = (event: ReactPointerEvent) => {
    const stageBox = stage.current?.getBoundingClientRect();
    if (!stageBox) return { x: 0, y: 0 };
    return {
      x: clamp((event.clientX - stageBox.left - box.left) / box.scale, width),
      y: clamp((event.clientY - stageBox.top - box.top) / box.scale, height),
    };
  };

  /**
   * A rectangle from a fixed corner to the pointer.
   *
   * With a ratio set, the smaller of the two edges wins — that way the shape
   * follows the hand instead of shooting off past the edge of the picture.
   */
  const spanned = (anchorX: number, anchorY: number, toX: number, toY: number): Rect => {
    const leftwards = toX < anchorX;
    const upwards = toY < anchorY;
    const maxWidth = leftwards ? anchorX : width - anchorX;
    const maxHeight = upwards ? anchorY : height - anchorY;
    let w = Math.min(Math.abs(toX - anchorX), maxWidth);
    let h = Math.min(Math.abs(toY - anchorY), maxHeight);
    if (ratio) {
      if (w / h > ratio) w = h * ratio;
      else h = w / ratio;
      if (w > maxWidth) {
        w = maxWidth;
        h = w / ratio;
      }
      if (h > maxHeight) {
        h = maxHeight;
        w = h * ratio;
      }
    }
    return {
      x: leftwards ? anchorX - w : anchorX,
      y: upwards ? anchorY - h : anchorY,
      width: w,
      height: h,
    };
  };

  const onDown = (event: ReactPointerEvent, kind: "draw" | "move", corner?: Rect) => {
    event.stopPropagation();
    event.preventDefault();
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    const point = at(event);
    if (kind === "move" && rect) {
      drag.current = { kind: "move", grabX: point.x - rect.x, grabY: point.y - rect.y };
    } else {
      // Resizing is drawing from the opposite corner — one piece of maths for
      // both, and the handles need no special case.
      const anchor = corner ?? point;
      drag.current = { kind: "draw", anchorX: anchor.x, anchorY: anchor.y };
      if (!corner) onRect({ x: point.x, y: point.y, width: 0, height: 0 });
    }
  };

  const onMove = (event: ReactPointerEvent) => {
    const state = drag.current;
    if (!state) return;
    const point = at(event);
    if (state.kind === "draw") {
      onRect(spanned(state.anchorX, state.anchorY, point.x, point.y));
    } else if (rect) {
      onRect({
        ...rect,
        x: clamp(point.x - state.grabX, width - rect.width),
        y: clamp(point.y - state.grabY, height - rect.height),
      });
    }
  };

  const onUp = (event: ReactPointerEvent) => {
    (event.currentTarget as HTMLElement).releasePointerCapture(event.pointerId);
    onRelease?.(rect);
    drag.current = null;
  };

  const percent = (value: number, of: number) => `${(value / of) * 100}%`;
  /** The corner opposite the handle — that is what a resize hangs from. */
  const corners: Array<[string, string, (r: Rect) => Rect]> = [
    ["topLeft", "left-0 top-0 -translate-x-1/2 -translate-y-1/2 cursor-nwse-resize",
      (r) => ({ ...r, x: r.x + r.width, y: r.y + r.height })],
    ["topRight", "right-0 top-0 translate-x-1/2 -translate-y-1/2 cursor-nesw-resize",
      (r) => ({ ...r, x: r.x, y: r.y + r.height })],
    ["bottomLeft", "left-0 bottom-0 -translate-x-1/2 translate-y-1/2 cursor-nesw-resize",
      (r) => ({ ...r, x: r.x + r.width, y: r.y })],
    ["bottomRight", "right-0 bottom-0 translate-x-1/2 translate-y-1/2 cursor-nwse-resize",
      (r) => ({ ...r, x: r.x, y: r.y })],
  ];

  return (
    <div
      className={cn("absolute touch-none", crop ? "cursor-crosshair" : "pointer-events-none")}
      style={{ left: box.left, top: box.top, width: box.width, height: box.height }}
      onPointerDown={crop ? (event) => onDown(event, "draw") : undefined}
      onPointerMove={onMove}
      onPointerUp={onUp}
    >
      {rect && rect.width > 0 && rect.height > 0 && (
        <div
          className={cn("absolute cursor-move", !crop && "pointer-events-auto")}
          style={{
            left: percent(rect.x, width),
            top: percent(rect.y, height),
            width: percent(rect.width, width),
            height: percent(rect.height, height),
            // Everything outside the selection goes dark. One shadow instead of
            // four panels — it can never leave a seam. As wide as the picture:
            // zoomed in, that is far beyond any fixed number.
            boxShadow: crop ? `0 0 0 ${Math.ceil(Math.max(box.width, box.height))}px rgba(0, 0, 0, 0.62)` : undefined,
            outline: crop ? "1px solid rgba(255, 255, 255, 0.9)" : "1px dashed rgba(255, 255, 255, 0.7)",
          }}
          onPointerDown={(event) => onDown(event, "move")}
          onPointerMove={onMove}
          onPointerUp={onUp}
        >
          {/* Thirds. They are the reason to crop by eye rather than by number. */}
          <div className={cn("pointer-events-none absolute inset-0 opacity-40", !crop && "hidden")}>
            <div className="absolute top-1/3 right-0 left-0 border-t border-white/60" />
            <div className="absolute top-2/3 right-0 left-0 border-t border-white/60" />
            <div className="absolute top-0 bottom-0 left-1/3 border-l border-white/60" />
            <div className="absolute top-0 bottom-0 left-2/3 border-l border-white/60" />
          </div>
          {corners.map(([key, placement, opposite]) => (
            <div
              key={key}
              className={cn(
                "absolute h-3.5 w-3.5 rounded-[3px] border border-black/40 bg-white",
                placement,
              )}
              onPointerDown={(event) => onDown(event, "draw", opposite(rect))}
              onPointerMove={onMove}
              onPointerUp={onUp}
            />
          ))}
        </div>
      )}
    </div>
  );
}
