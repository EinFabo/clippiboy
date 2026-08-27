import {
  memo,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";

import { usePictureBox, type PictureBox } from "@/lib/pictureBox";
import { cn } from "@/lib/cn";

export type Tool = "select" | "arrow" | "box" | "ellipse" | "pen" | "text" | "blur";

export const TOOLS: Array<[Tool, string]> = [
  ["select", "Select"],
  ["arrow", "Arrow"],
  ["box", "Box"],
  ["ellipse", "Ellipse"],
  ["pen", "Pen"],
  ["text", "Text"],
  ["blur", "Blur"],
];

/** Straight from the tokens, so a marked-up picture looks like the app. */
export const COLORS = [
  "#ef4444",
  "#f59e0b",
  "#34d399",
  "#a78bfa",
  "#ffffff",
  "#08080a",
];

/**
 * One drawn thing, in pixels of the picture.
 *
 * Never in pixels of the screen: the same mark has to sit in the same place
 * after the window is resized, and the file knows nothing about how big the
 * preview was.
 */
export interface Shape {
  id: number;
  tool: Exclude<Tool, "select">;
  color: string;
  /**
   * The tool's one number, in picture pixels: line thickness for arrow, box,
   * ellipse and pen, height for text, radius for blur. One field because every
   * tool has exactly one, and one slider then serves them all.
   */
  size: number;
  /** Box and ellipse: filled in. */
  fill?: boolean;
  /** Box and ellipse: drawn with a border. Both together is a filled shape
   *  with a rim; neither would be nothing at all, so the panel offers three
   *  choices rather than two switches. */
  outline?: boolean;
  /** Blur: an oval inside the rectangle instead of the rectangle itself. */
  round?: boolean;
  /**
   * The second ink: the border of a filled box, the rim around a text. Unset
   * means the sensible default — the same colour for a border, the opposite
   * shade for a rim.
   */
  altColor?: string;
  /** Freehand only. */
  points?: Array<[number, number]>;
  /** Text only. */
  text?: string;
  x1: number;
  y1: number;
  x2: number;
  y2: number;
}

/** What a new shape of this tool starts out as. */
export interface Style {
  color: string;
  /** See {@link Shape.altColor}. */
  altColor: string;
  size: number;
  fill: boolean;
  outline: boolean;
  round: boolean;
}

/** How a box or an ellipse is painted. */
export const LOOKS: Array<[string, { fill: boolean; outline: boolean }]> = [
  ["Outline", { fill: false, outline: true }],
  ["Filled", { fill: true, outline: false }],
  ["Both", { fill: true, outline: true }],
];

/** What shape a blur takes. */
export const BLUR_SHAPES: Array<[string, boolean]> = [
  ["Rectangle", false],
  ["Ellipse", true],
];

/** Which of the three looks a shape currently has. */
export function lookOf(style: { fill: boolean; outline: boolean }): string {
  if (style.fill && style.outline) return "Both";
  return style.fill ? "Filled" : "Outline";
}

/**
 * The thickness of a stroke at its default on this picture.
 *
 * Relative to the picture, so a mark looks the same on a 4K still as on a 1080p
 * one — a fixed four pixels would be a hairline on the one and a bar on the
 * other.
 */
export function baseStroke(width: number): number {
  return Math.max(2, Math.round(width / 480));
}

/** Where the slider for this tool starts, ends and stands to begin with. */
export function range(tool: Tool, pictureWidth: number): [number, number, number] {
  const base = baseStroke(pictureWidth);
  switch (tool) {
    case "text":
      return [base * 3, base * 24, base * 7];
    case "blur":
      return [base, base * 12, base * 2];
    default:
      return [Math.max(1, base * 0.4), base * 6, base];
  }
}

/** Does this tool paint in a colour at all? Blur has none to pick. */
export function hasColor(tool: Tool): boolean {
  return tool !== "blur";
}

/** Can this tool be filled in, outlined, or both? */
export function hasLook(tool: Tool): boolean {
  return tool === "box" || tool === "ellipse";
}

/**
 * Does this tool paint in two colours right now?
 *
 * Text always does — the rim is what keeps it readable. A box or an ellipse
 * only once it is both filled and outlined; with one of the two there is one
 * ink, and a second control would set something invisible.
 */
export function hasAltColor(tool: Tool, style: { fill: boolean; outline: boolean }): boolean {
  if (tool === "text") return true;
  return (tool === "box" || tool === "ellipse") && style.fill && style.outline;
}

/** Can this tool be a rectangle or an oval? */
export function hasShape(tool: Tool): boolean {
  return tool === "blur";
}

/** What the slider is called for this tool. */
export function sizeLabel(tool: Tool): string {
  if (tool === "text") return "Text size";
  if (tool === "blur") return "Blur";
  return "Thickness";
}

/**
 * Draw everything onto a context in picture coordinates.
 *
 * The same routine serves the preview and the file: the canvas over the picture
 * *is* what gets sent, so what you see cannot drift from what is written. The
 * selection's handles are deliberately not in here — they are DOM, and would
 * otherwise end up in the picture.
 */
export function paint(ctx: CanvasRenderingContext2D, shapes: Shape[]) {
  ctx.lineCap = "round";
  ctx.lineJoin = "round";

  for (const shape of shapes) {
    ctx.strokeStyle = shape.color;
    ctx.fillStyle = shape.color;
    ctx.lineWidth = shape.size;
    ctx.globalAlpha = 1;

    const { x1, y1, x2, y2 } = shape;
    const left = Math.min(x1, x2);
    const top = Math.min(y1, y2);
    const width = Math.abs(x2 - x1);
    const height = Math.abs(y2 - y1);

    switch (shape.tool) {
      case "box":
        if (shape.fill) ctx.fillRect(left, top, width, height);
        // Undefined means an older mark from before there was a choice, and
        // those were all outlines.
        if (shape.outline !== false) {
          ctx.strokeStyle = outlineOf(shape);
          ctx.strokeRect(left, top, width, height);
        }
        break;

      case "ellipse": {
        if (width < 2 || height < 2) break;
        ctx.beginPath();
        ctx.ellipse(left + width / 2, top + height / 2, width / 2, height / 2, 0, 0, Math.PI * 2);
        if (shape.fill) ctx.fill();
        if (shape.outline !== false) {
          ctx.strokeStyle = outlineOf(shape);
          ctx.stroke();
        }
        break;
      }

      case "arrow": {
        const angle = Math.atan2(y2 - y1, x2 - x1);
        // Long enough to read at a glance, and it grows with the stroke.
        const head = Math.max(shape.size * 4, 10);
        ctx.beginPath();
        ctx.moveTo(x1, y1);
        ctx.lineTo(x2, y2);
        ctx.stroke();
        ctx.beginPath();
        ctx.moveTo(x2, y2);
        ctx.lineTo(
          x2 - head * Math.cos(angle - Math.PI / 7),
          y2 - head * Math.sin(angle - Math.PI / 7),
        );
        ctx.lineTo(
          x2 - head * Math.cos(angle + Math.PI / 7),
          y2 - head * Math.sin(angle + Math.PI / 7),
        );
        ctx.closePath();
        ctx.fill();
        break;
      }

      case "pen": {
        const points = shape.points ?? [];
        if (points.length < 2) break;
        ctx.beginPath();
        ctx.moveTo(points[0][0], points[0][1]);
        for (const [x, y] of points.slice(1)) ctx.lineTo(x, y);
        ctx.stroke();
        break;
      }

      case "text": {
        if (!shape.text) break;
        ctx.font = textFont(shape.size);
        ctx.textBaseline = "top";
        // A rim in the opposite shade — red text on a red HUD is invisible
        // otherwise, and so is white on snow.
        ctx.lineWidth = Math.max(2, shape.size / 10);
        ctx.strokeStyle = shape.altColor ?? rimColor(shape.color);
        // Line by line, and only where a newline was actually typed: the field
        // does not wrap either, so both agree on where a line ends.
        shape.text.split("\n").forEach((line, at) => {
          const y = y1 + at * shape.size * LINE_HEIGHT;
          ctx.strokeText(line, x1, y);
          ctx.fillText(line, x1, y);
        });
        break;
      }

      case "blur":
        // Not here: blurring reads what is underneath, and this layer can only
        // cover. The rectangles travel to the core on their own.
        break;
    }
  }
  ctx.globalAlpha = 1;
}

/**
 * The ink a border is drawn in.
 *
 * Only a filled shape has two of them; without a fill the border **is** the
 * shape, and its one colour is the one that was picked. Otherwise the panel
 * would show a colour that changes nothing — the border would quietly keep
 * whatever the second colour happened to be.
 */
function outlineOf(shape: Shape): string {
  return shape.fill ? (shape.altColor ?? shape.color) : shape.color;
}

function textFont(size: number): string {
  return `600 ${size}px Inter, "Segoe UI", sans-serif`;
}

/** Line spacing, shared by the canvas and the field you type in. */
export const LINE_HEIGHT = 1.25;

/** The rim colour that keeps text readable on any ground. */
export function rimColor(color: string): string {
  return color === "#08080a" ? "#ffffff" : "#08080a";
}

/** The box a shape occupies, in picture pixels. */
export function bounds(
  shape: Shape,
  measure?: CanvasRenderingContext2D,
): { x: number; y: number; width: number; height: number } {
  if (shape.tool === "text") {
    const lines = (shape.text ?? "").split("\n");
    let width = Math.max(...lines.map((line) => line.length)) * shape.size * 0.55;
    if (measure) {
      measure.font = textFont(shape.size);
      width = Math.max(...lines.map((line) => measure.measureText(line).width));
    }
    return {
      x: shape.x1,
      y: shape.y1,
      width,
      height: lines.length * shape.size * LINE_HEIGHT,
    };
  }
  if (shape.tool === "pen" && shape.points?.length) {
    const xs = shape.points.map(([x]) => x);
    const ys = shape.points.map(([, y]) => y);
    return {
      x: Math.min(...xs),
      y: Math.min(...ys),
      width: Math.max(...xs) - Math.min(...xs),
      height: Math.max(...ys) - Math.min(...ys),
    };
  }
  return {
    x: Math.min(shape.x1, shape.x2),
    y: Math.min(shape.y1, shape.y2),
    width: Math.abs(shape.x2 - shape.x1),
    height: Math.abs(shape.y2 - shape.y1),
  };
}

/**
 * Which shape is under this point — the topmost one wins.
 *
 * Generous on purpose: a two-pixel arrow is impossible to hit exactly, so
 * everything gets a margin of the stroke it was drawn with.
 */
function hit(
  shapes: Shape[],
  x: number,
  y: number,
  slack: number,
  measure?: CanvasRenderingContext2D,
): Shape | null {
  for (let at = shapes.length - 1; at >= 0; at--) {
    const shape = shapes[at];
    // A text's box is the text: its `size` is the height of a line, and taking
    // that as a halo would let one caption swallow every click near it.
    const margin = shape.tool === "text" ? slack : Math.max(slack, shape.size);
    if (shape.tool === "arrow") {
      if (nearLine(x, y, shape.x1, shape.y1, shape.x2, shape.y2) <= margin) return shape;
      continue;
    }
    if (shape.tool === "pen") {
      const points = shape.points ?? [];
      if (points.some(([px, py]) => Math.hypot(px - x, py - y) <= margin)) return shape;
      continue;
    }
    const box = bounds(shape, measure);
    if (
      x >= box.x - margin &&
      x <= box.x + box.width + margin &&
      y >= box.y - margin &&
      y <= box.y + box.height + margin
    ) {
      return shape;
    }
  }
  return null;
}

/** Distance from a point to a line segment. */
function nearLine(
  x: number,
  y: number,
  x1: number,
  y1: number,
  x2: number,
  y2: number,
): number {
  const dx = x2 - x1;
  const dy = y2 - y1;
  const length = dx * dx + dy * dy;
  const along = length === 0 ? 0 : Math.min(1, Math.max(0, ((x - x1) * dx + (y - y1) * dy) / length));
  return Math.hypot(x - (x1 + along * dx), y - (y1 + along * dy));
}

/**
 * Which corners a shape can be pulled by. `move` alone means: none.
 *
 * Measured the same way the frame around it is — a text has no width until a
 * font has been applied to it, and a handle reckoned without one sits somewhere
 * else than the dashed line it belongs to.
 */
function handlesOf(
  shape: Shape,
  measure?: CanvasRenderingContext2D,
): Array<[string, number, number]> {
  if (shape.tool === "arrow") {
    return [
      ["start", shape.x1, shape.y1],
      ["end", shape.x2, shape.y2],
    ];
  }
  if (shape.tool === "pen") return [];
  const box = bounds(shape, measure);
  if (shape.tool === "text") {
    // One handle, and it scales rather than stretches — text has a size, not a
    // width and a height.
    return [["scale", box.x + box.width, box.y + box.height]];
  }
  return [
    ["topLeft", box.x, box.y],
    ["topRight", box.x + box.width, box.y],
    ["bottomLeft", box.x, box.y + box.height],
    ["bottomRight", box.x + box.width, box.y + box.height],
  ];
}

/**
 * One step of the annotation, in the order it was drawn.
 *
 * A layer covers what is under it, a blur reads it — so the two cannot be
 * merged into one picture, and the order between them has to be kept. Marks
 * drawn one after another share a layer, so the usual run is a single one.
 */
export type Step =
  | {
      kind: "blur";
      x: number;
      y: number;
      width: number;
      height: number;
      radius: number;
      ellipse: boolean;
    }
  | { kind: "layer"; shapes: Shape[] };

export function toSteps(shapes: Shape[]): Step[] {
  const steps: Step[] = [];
  let run: Shape[] = [];
  const flush = () => {
    if (run.length) steps.push({ kind: "layer", shapes: run });
    run = [];
  };

  for (const shape of shapes) {
    if (shape.tool !== "blur") {
      run.push(shape);
      continue;
    }
    flush();
    const frame = bounds(shape);
    if (frame.width < 2 || frame.height < 2) continue;
    steps.push({
      kind: "blur",
      x: Math.round(frame.x),
      y: Math.round(frame.y),
      width: Math.round(frame.width),
      height: Math.round(frame.height),
      // The radius the preview blurred by, so the file matches it.
      radius: Math.round(shape.size),
      ellipse: shape.round ?? false,
    });
  }
  flush();
  return steps;
}

/**
 * A run of marks, painted onto a canvas of its own.
 *
 * Wrapped in `memo` on purpose: the canvas carries the picture's full
 * resolution, and on a 4K still clearing and repainting one costs a good part
 * of a frame. Only the layer whose marks really changed may do that.
 */
const ShapeLayer = memo(function ShapeLayer({
  shapes,
  width,
  height,
  box,
}: {
  shapes: Shape[];
  width: number;
  height: number;
  box: PictureBox;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const element = canvas.current;
    const ctx = element?.getContext("2d");
    if (!element || !ctx) return;
    ctx.clearRect(0, 0, element.width, element.height);
    paint(ctx, shapes);
  }, [shapes, width, height]);

  return (
    <canvas
      ref={canvas}
      width={width}
      height={height}
      className="pointer-events-none absolute"
      style={{ left: box.left, top: box.top, width: box.width, height: box.height }}
    />
  );
});

interface Props {
  stage: React.RefObject<HTMLDivElement | null>;
  width: number;
  height: number;
  tool: Tool;
  style: Style;
  shapes: Shape[];
  onShapes: (shapes: Shape[]) => void;
  selected: number | null;
  onSelect: (id: number | null) => void;
}

/**
 * The drawing layer over the picture.
 *
 * The canvas holds the picture's own resolution and is only scaled down by CSS,
 * so a mark is as sharp in the file as it looks here — and the export is this
 * very canvas, not a second rendering of it. Everything that must **not** end up
 * in the file — the selection frame, the handles, the blur preview — is DOM
 * beside it.
 */
export function AnnotateLayer({
  stage,
  width,
  height,
  tool,
  style,
  shapes,
  onShapes,
  selected,
  onSelect,
}: Props) {
  const box = usePictureBox(stage, width, height);
  const [draft, setDraft] = useState<Shape | null>(null);
  /** Where a text is being typed, and which shape it belongs to. */
  const [typing, setTyping] = useState<{ x: number; y: number; id: number | null; value: string } | null>(
    null,
  );
  const next = useRef(1);
  /** What the pointer is doing. Kept in a ref: it changes per frame. */
  const drag = useRef<
    | { kind: "move"; id: number; grabX: number; grabY: number; from: Shape }
    | { kind: "handle"; id: number; handle: string; from: Shape }
    | null
  >(null);

  // Ids have to clear whatever is already there, or an undo plus a new mark
  // would collide.
  useEffect(() => {
    next.current = Math.max(next.current, ...shapes.map((shape) => shape.id + 1), 1);
  }, [shapes]);

  const field = useRef<HTMLTextAreaElement>(null);

  // Focus by hand rather than with `autoFocus`: the browser scrolls a freshly
  // focused element into view, and inside the stage that shoves the whole
  // picture sideways. `preventScroll` is the whole fix.
  useEffect(() => {
    if (typing) field.current?.focus({ preventScroll: true });
  }, [typing?.id, typing?.x, typing?.y]);

  // The field grows with what is typed, in both directions. It does not wrap —
  // a line ends where a newline was typed, exactly as on the canvas.
  useLayoutEffect(() => {
    const element = field.current;
    if (!element) return;
    element.style.width = "0px";
    element.style.width = `${element.scrollWidth + 2}px`;
    element.style.height = "0px";
    element.style.height = `${element.scrollHeight}px`;
  }, [typing?.value, typing?.id]);

  // What was drawn last lies on top — including a blur, which then covers the
  // arrow underneath it. In the DOM that is simply the order they are written
  // in, and `backdrop-filter` picks up everything painted before it.
  //
  // The mark under the hand right now gets a layer of its own rather than
  // joining the run before it. It changes with every movement, and sharing a
  // canvas would repaint every finished mark along with it — at 4K a
  // full-resolution canvas per frame, for every layer there is.
  const layers = useMemo(() => toSteps(shapes), [shapes]);
  const drawing = useMemo(() => (draft ? toSteps([draft]) : []), [draft]);

  if (!box) return null;

  const measure = measuring();
  const chosen = shapes.find((shape) => shape.id === selected) ?? null;
  const slack = 6 / box.scale;

  const at = (event: ReactPointerEvent) => {
    const rect = stage.current?.getBoundingClientRect();
    if (!rect) return { x: 0, y: 0 };
    const clamp = (value: number, max: number) => Math.min(Math.max(value, 0), max);
    return {
      x: clamp((event.clientX - rect.left - box.left) / box.scale, width),
      y: clamp((event.clientY - rect.top - box.top) / box.scale, height),
    };
  };

  const replace = (id: number, patch: Partial<Shape>) =>
    onShapes(shapes.map((shape) => (shape.id === id ? { ...shape, ...patch } : shape)));

  const onDown = (event: ReactPointerEvent) => {
    // A click beside an open text field puts that text down and does nothing
    // else. It cannot be left to `blur`: the `preventDefault` below suppresses
    // the focus change that would fire it — and even if it fired, the same
    // click would immediately start the next mark.
    if (typing) {
      event.preventDefault();
      commitText(typing.value);
      return;
    }

    event.preventDefault();
    const point = at(event);
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);

    // Picking up an existing mark is its own tool. Otherwise you could never
    // draw a box over a box — the first one would swallow every click meant for
    // the second.
    if (tool === "select") {
      const under = hit(shapes, point.x, point.y, slack, measure);
      onSelect(under?.id ?? null);
      if (under) {
        drag.current = {
          kind: "move",
          id: under.id,
          grabX: point.x,
          grabY: point.y,
          from: under,
        };
      }
      return;
    }

    if (tool === "text") {
      setTyping({ x: point.x, y: point.y, id: null, value: "" });
      return;
    }

    onSelect(null);
    setDraft({
      id: next.current++,
      tool,
      color: style.color,
      altColor: style.altColor,
      size: style.size,
      fill: style.fill,
      outline: style.outline,
      round: style.round,
      x1: point.x,
      y1: point.y,
      x2: point.x,
      y2: point.y,
      points: tool === "pen" ? [[point.x, point.y]] : undefined,
    });
  };

  const onMove = (event: ReactPointerEvent) => {
    const state = drag.current;
    const point = at(event);

    if (state?.kind === "move") {
      const from = state.from;
      // The mark has to stay in the picture. `at()` clamps the pointer, but the
      // distance it has travelled does not: grab a mark at x=500, pull to the
      // left edge, and the mark lands at −100. The core stores those
      // coordinates unsigned, so a single negative one makes the **whole** save
      // fail — crop and every other mark with it — and all the user sees is a
      // toast. So the shift is limited to what the mark's own extent allows.
      const xs = [from.x1, from.x2, ...(from.points?.map(([x]) => x) ?? [])];
      const ys = [from.y1, from.y2, ...(from.points?.map(([, y]) => y) ?? [])];
      const shift = (raw: number, low: number, high: number) =>
        Math.min(Math.max(raw, -low), high);
      const dx = shift(point.x - state.grabX, Math.min(...xs), width - Math.max(...xs));
      const dy = shift(point.y - state.grabY, Math.min(...ys), height - Math.max(...ys));
      replace(state.id, {
        x1: from.x1 + dx,
        y1: from.y1 + dy,
        x2: from.x2 + dx,
        y2: from.y2 + dy,
        points: from.points?.map(([x, y]) => [x + dx, y + dy] as [number, number]),
      });
      return;
    }

    if (state?.kind === "handle") {
      const from = state.from;
      if (from.tool === "arrow") {
        replace(
          state.id,
          state.handle === "start"
            ? { x1: point.x, y1: point.y }
            : { x2: point.x, y2: point.y },
        );
        return;
      }
      if (from.tool === "text") {
        // The distance from the anchor sets the size — pulling away makes it
        // bigger, and the text never leaves its corner.
        const lines = (from.text ?? "").split("\n").length;
        const grown = Math.max(6, point.y - from.y1);
        replace(state.id, { size: Math.max(6, Math.round(grown / (lines * LINE_HEIGHT))) });
        return;
      }
      const box = bounds(from);
      const anchorX = state.handle.endsWith("Left") ? box.x + box.width : box.x;
      const anchorY = state.handle.startsWith("top") ? box.y + box.height : box.y;
      replace(state.id, { x1: anchorX, y1: anchorY, x2: point.x, y2: point.y });
      return;
    }

    if (!draft) return;
    setDraft(
      draft.tool === "pen"
        ? { ...draft, points: [...(draft.points ?? []), [point.x, point.y]] }
        : { ...draft, x2: point.x, y2: point.y },
    );
  };

  const onUp = (event: ReactPointerEvent) => {
    (event.currentTarget as HTMLElement).releasePointerCapture(event.pointerId);
    drag.current = null;
    if (!draft) return;
    // A click that never moved is a slip, not a mark.
    const moved =
      draft.tool === "pen"
        ? (draft.points?.length ?? 0) > 2
        : Math.abs(draft.x2 - draft.x1) + Math.abs(draft.y2 - draft.y1) > draft.size;
    if (moved) {
      onShapes([...shapes, draft]);
      onSelect(draft.id);
    }
    setDraft(null);
  };

  const commitText = (value: string) => {
    if (!typing) return;
    const text = value.trim();
    if (typing.id !== null) {
      // Emptied out means gone — that is how you delete a caption.
      onShapes(
        text
          ? shapes.map((shape) => (shape.id === typing.id ? { ...shape, text } : shape))
          : shapes.filter((shape) => shape.id !== typing.id),
      );
    } else if (text) {
      const shape: Shape = {
        id: next.current++,
        tool: "text",
        color: style.color,
        altColor: style.altColor,
        size: style.size,
        x1: typing.x,
        y1: typing.y,
        x2: typing.x,
        y2: typing.y,
        text,
      };
      onShapes([...shapes, shape]);
      onSelect(shape.id);
    }
    setTyping(null);
  };

  const css = (x: number, y: number) => ({
    left: box.left + x * box.scale,
    top: box.top + y * box.scale,
  });

  const asLayer = (step: Step, key: string) =>
    step.kind === "blur" ? (
      <BlurPatch key={key} step={step} box={box} />
    ) : (
      <ShapeLayer key={key} shapes={step.shapes} width={width} height={height} box={box} />
    );

  return (
    <>
      {layers.map((step, at) => asLayer(step, `layer-${at}`))}
      {drawing.map((step, at) => asLayer(step, `draft-${at}`))}

      <div
        className={cn(
          "absolute touch-none",
          tool === "select" ? "cursor-default" : "cursor-crosshair",
        )}
        style={{ left: box.left, top: box.top, width: box.width, height: box.height }}
        onPointerDown={onDown}
        onPointerMove={onMove}
        onPointerUp={onUp}
        onDoubleClick={(event) => {
          const rect = stage.current?.getBoundingClientRect();
          if (!rect) return;
          const x = (event.clientX - rect.left - box.left) / box.scale;
          const y = (event.clientY - rect.top - box.top) / box.scale;
          const under = hit(shapes, x, y, slack, measure);
          if (under?.tool === "text") {
            setTyping({ x: under.x1, y: under.y1, id: under.id, value: under.text ?? "" });
          }
        }}
      />

      {chosen && !typing && (
        <>
          {(() => {
            const frame = bounds(chosen, measure);
            const pad = 6 / box.scale;
            return (
              <div
                className="pointer-events-none absolute rounded-[3px] border border-dashed border-white/70"
                style={{
                  ...css(frame.x - pad, frame.y - pad),
                  width: (frame.width + pad * 2) * box.scale,
                  height: (frame.height + pad * 2) * box.scale,
                }}
              />
            );
          })()}
          {handlesOf(chosen, measure).map(([key, x, y]) => (
            <div
              key={key}
              className="absolute h-3 w-3 -translate-x-1/2 -translate-y-1/2 cursor-pointer
                rounded-[3px] border border-black/50 bg-white"
              style={css(x, y)}
              onPointerDown={(event) => {
                event.stopPropagation();
                event.preventDefault();
                (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
                drag.current = { kind: "handle", id: chosen.id, handle: key, from: chosen };
              }}
              onPointerMove={onMove}
              onPointerUp={(event) => {
                (event.currentTarget as HTMLElement).releasePointerCapture(event.pointerId);
                drag.current = null;
              }}
            />
          ))}
        </>
      )}

      {typing && (
        // Typed where it will stand, in the size and colour it will have: the
        // field carries the same font, the same line spacing and the same rim
        // as the canvas, so there is nothing to imagine.
        <textarea
          ref={field}
          aria-label="Text on the picture"
          spellCheck={false}
          wrap="off"
          value={typing.value}
          onChange={(event) => setTyping({ ...typing, value: event.target.value })}
          onBlur={(event) => commitText(event.target.value)}
          onKeyDown={(event) => {
            // Enter makes a line, like anywhere else you write. Escape and
            // Ctrl+Enter are done — nothing here throws away what was typed,
            // an empty text removes the mark instead.
            if (event.key === "Escape" || (event.key === "Enter" && event.ctrlKey)) {
              event.preventDefault();
              commitText(event.currentTarget.value);
            }
            event.stopPropagation();
          }}
          className="absolute resize-none overflow-hidden bg-transparent p-0
            whitespace-pre outline-none"
          style={{
            ...css(typing.x, typing.y),
            color: style.color,
            font: `600 ${Math.max(9, style.size * box.scale)}px Inter, "Segoe UI", sans-serif`,
            lineHeight: LINE_HEIGHT,
            WebkitTextStroke: `${Math.max(2, style.size / 10) * box.scale}px ${style.altColor}`,
            // The canvas strokes first and fills over it. Without this the
            // stroke would sit on top of the glyph and thin it out.
            paintOrder: "stroke",
            caretColor: style.color,
            // A hint that this is a field, without a box that would cover the
            // picture you are annotating.
            outline: "1px dashed rgba(255, 255, 255, 0.35)",
            outlineOffset: 4,
            minWidth: 8,
          }}
        />
      )}
    </>
  );
}

/**
 * A blurred area in the preview.
 *
 * `backdrop-filter` blurs what is painted behind it — that is the picture
 * itself, so this is not a stand-in for the result but the result, at preview
 * size. The core blurs by the same radius.
 */
function BlurPatch({
  step,
  box,
}: {
  step: Extract<Step, { kind: "blur" }>;
  box: PictureBox;
}) {
  return (
    <div
      className="pointer-events-none absolute"
      style={{
        left: box.left + step.x * box.scale,
        top: box.top + step.y * box.scale,
        width: step.width * box.scale,
        height: step.height * box.scale,
        // The rounding clips the backdrop with it, so an oval here is an oval
        // in the file — the core masks the same way.
        borderRadius: step.ellipse ? "50%" : undefined,
        backdropFilter: `blur(${sigmaOf(step.radius) * box.scale}px)`,
      }}
    />
  );
}

/**
 * The radius CSS has to be given so the preview blurs as hard as the file.
 *
 * `blur()` is a Gaussian and takes a standard deviation. The core runs **one**
 * box pass of half-width `radius` per axis, and blurring is separable — so per
 * axis it is a single box, whose variance is `r·(r+1)/3`. Counting both passes
 * onto the same axis gives `2·r·(r+1)/3` and makes the preview a good 40 %
 * softer than what is written; for a redacted name that is exactly the wrong
 * direction to be wrong in.
 */
function sigmaOf(radius: number): number {
  return Math.sqrt((radius * (radius + 1)) / 3);
}

/**
 * A context used only to measure text.
 *
 * Text has no width until a font has been applied to it, and the selection
 * frame needs one. A canvas of its own for that, so measuring never disturbs
 * what is being drawn.
 */
let ruler: CanvasRenderingContext2D | null = null;
function measuring(): CanvasRenderingContext2D | undefined {
  if (!ruler) ruler = document.createElement("canvas").getContext("2d");
  return ruler ?? undefined;
}
