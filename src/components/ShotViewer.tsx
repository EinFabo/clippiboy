import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { createPortal } from "react-dom";

import { Button } from "@/components/ui/Button";
import { Pill } from "@/components/ui/Card";
import { ConfirmDelete } from "@/components/ui/ConfirmDelete";
import { HeartBurst } from "@/components/ui/HeartBurst";
import { ClipMeta } from "@/components/ClipMeta";
import { useClipMenu } from "@/components/clipMenu";
import { IconTrash } from "@/components/icons";
import { api, fileUrl, inTauri } from "@/lib/ipc";
import { formatAgo, formatSize } from "@/lib/format";
import { EASE_EXIT, prefersReducedMotion } from "@/lib/motion";
import { cn } from "@/lib/cn";
import { useEngine } from "@/store";
import type { Clip } from "@/lib/types";

/** Has to match `cb-player-out` in styles/motion.css. */
const LEAVE_MS = 200;

/** Below this a crop is a slip of the hand, not an intention. */
const MIN_EDGE = 16;

/** A rectangle in pixels of the picture — never of the preview. */
interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * The shapes to crop to. `Free` holds no ratio at all, `Original` keeps the
 * picture's own — useful for shrinking without changing the shape. The rest
 * read as `w:h` and are parsed as such, so the list stays a list.
 */
const PRESETS = [
  "Free",
  "Original",
  "16:9",
  "9:16",
  "4:3",
  "3:2",
  "1:1",
  "21:9",
  "4:5",
] as const;

function ratioOf(preset: string, width: number, height: number): number | null {
  if (preset === "Free") return null;
  if (preset === "Original") return width / height;
  const [w, h] = preset.split(":").map(Number);
  return w / h;
}

/**
 * Reshape a selection to a ratio, keeping its centre.
 *
 * Shrinking, never growing: the picture is the limit, and a selection that grew
 * to meet its ratio would run over the edge on the very first click.
 */
function fitRatio(rect: Rect, ratio: number, width: number, height: number): Rect {
  let w = rect.width;
  let h = rect.height;
  if (w / h > ratio) w = h * ratio;
  else h = w / ratio;
  if (w > width) {
    w = width;
    h = w / ratio;
  }
  if (h > height) {
    h = height;
    w = h * ratio;
  }
  const centreX = rect.x + rect.width / 2;
  const centreY = rect.y + rect.height / 2;
  return {
    x: Math.min(Math.max(centreX - w / 2, 0), width - w),
    y: Math.min(Math.max(centreY - h / 2, 0), height - h),
    width: w,
    height: h,
  };
}

interface Props {
  clips: Clip[];
  index: number;
  onIndexChange: (index: number) => void;
  onClose: () => void;
  onDelete: (id: string) => void;
  /** Where the tile sits on screen, for the shrink-back on close. */
  originOf?: (clipId: string) => DOMRect | null;
}

/**
 * The viewer for screenshots, with the crop built in.
 *
 * A file of its own rather than a branch inside `ClipPlayer`: that one is a
 * thousand lines about duration, trim handles, separate audio tracks and a
 * waveform, and a still has none of those. What the two share — the name, the
 * description and the game — sits in {@link ClipMeta}.
 */
export function ShotViewer({
  clips,
  index,
  onIndexChange,
  onClose,
  onDelete,
  originOf,
}: Props) {
  const clip = clips[index];
  const setFavorite = useEngine((state) => state.setFavorite);
  const cropScreenshot = useEngine((state) => state.cropScreenshot);
  const restoreScreenshot = useEngine((state) => state.restoreScreenshot);
  const clipMenu = useClipMenu();

  const [leaving, setLeaving] = useState(false);
  const [shrinking, setShrinking] = useState(false);
  const [askingDelete, setAskingDelete] = useState(false);
  const [broken, setBroken] = useState(false);
  const [cropping, setCropping] = useState(false);
  const [ratio, setRatio] = useState<string>("Free");
  const [sel, setSel] = useState<Rect | null>(null);
  const [busy, setBusy] = useState(false);
  /** Is the untouched picture still beside this one? Only the core knows. */
  const [hasOriginal, setHasOriginal] = useState(false);
  const frame = useRef<HTMLDivElement>(null);
  const goodbye = useRef<number | undefined>(undefined);

  /** Play the exit, then really go. Everything that closes goes through here. */
  const close = useCallback(() => {
    if (prefersReducedMotion()) {
      onClose();
      return;
    }
    // The stage travels back onto the tile it came from, like the player's.
    const stage = frame.current;
    const target = clip ? originOf?.(clip.id) : null;
    if (stage && target && target.width > 0) {
      const box = stage.getBoundingClientRect();
      stage.style.transformOrigin = "top left";
      stage.animate(
        [
          { transform: "none" },
          {
            transform:
              `translate(${target.left - box.left}px, ${target.top - box.top}px) ` +
              `scale(${target.width / box.width}, ${target.height / box.height})`,
          },
        ],
        { duration: LEAVE_MS, easing: EASE_EXIT, fill: "forwards" },
      );
      setShrinking(true);
    }
    setLeaving((already) => {
      if (already) return already;
      goodbye.current = window.setTimeout(onClose, LEAVE_MS);
      return true;
    });
  }, [clip, onClose, originOf]);

  useEffect(() => () => clearTimeout(goodbye.current), []);

  /** Delete and move on — if the last picture is gone, there is nothing left. */
  const removeClip = useCallback(() => {
    if (!clip) return;
    setAskingDelete(false);
    onDelete(clip.id);
    if (clips.length <= 1) close();
    else onIndexChange(Math.min(index, clips.length - 2));
  }, [clip, clips.length, index, close, onDelete, onIndexChange]);

  const step = useCallback(
    (delta: number) => {
      const next = index + delta;
      if (next >= 0 && next < clips.length) onIndexChange(next);
    },
    [clips.length, index, onIndexChange],
  );

  const id = clip?.id;

  // Everything about the previous picture says nothing about this one.
  useEffect(() => {
    setBroken(false);
    setCropping(false);
    setSel(null);
    setHasOriginal(false);
    if (!id || !inTauri) return;
    let current = true;
    api
      .screenshotHasOriginal(id)
      .then((yes) => current && setHasOriginal(yes))
      .catch(() => {});
    return () => {
      current = false;
    };
  }, [id]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      // While a name is being typed the keys belong to the field.
      const target = event.target as HTMLElement | null;
      if (target && /^(INPUT|TEXTAREA)$/.test(target.tagName)) return;
      if (event.key === "Escape") {
        if (askingDelete) return; // the question takes it — see ConfirmDelete
        // One step at a time: Escape leaves the crop, and only the next one
        // closes the viewer. Otherwise a mis-drawn rectangle would cost the
        // whole picture you were looking at.
        if (cropping) cancelCrop();
        else close();
      } else if (!cropping && event.key === "ArrowLeft") {
        step(-1);
      } else if (!cropping && event.key === "ArrowRight") {
        step(1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [askingDelete, close, cropping, step]);

  if (!clip) return null;

  // The cache buster is the size: after a crop there is a new picture under the
  // same path, and the WebView would otherwise keep showing the old one.
  const source = inTauri ? `${fileUrl(clip.path)}?v=${clip.sizeBytes}` : null;

  /**
   * A preset both starts the crop and shapes it.
   *
   * From standing still that means one click for the whole job: the selection
   * begins as the whole picture, and the ratio trims it to shape around its
   * centre. Picking another preset reshapes what is already selected.
   */
  const pickPreset = (preset: string) => {
    const base = cropping && sel
      ? sel
      : { x: 0, y: 0, width: clip.width, height: clip.height };
    const wanted = ratioOf(preset, clip.width, clip.height);
    setRatio(preset);
    setSel(wanted ? fitRatio(base, wanted, clip.width, clip.height) : base);
    setCropping(true);
  };

  const cancelCrop = () => {
    setCropping(false);
    setSel(null);
  };

  const applyCrop = async () => {
    if (!sel) return;
    setBusy(true);
    try {
      await cropScreenshot(
        clip.id,
        Math.round(sel.x),
        Math.round(sel.y),
        Math.round(sel.width),
        Math.round(sel.height),
      );
      setHasOriginal(true);
      setCropping(false);
      setSel(null);
    } catch {
      // The notice is already in the store.
    } finally {
      setBusy(false);
    }
  };

  const undoCrop = async () => {
    setBusy(true);
    try {
      await restoreScreenshot(clip.id);
      setHasOriginal(false);
    } catch {
      // The notice is already in the store.
    } finally {
      setBusy(false);
    }
  };

  const whole =
    !sel ||
    (sel.x === 0 &&
      sel.y === 0 &&
      Math.round(sel.width) >= clip.width &&
      Math.round(sel.height) >= clip.height);

  // Hung under <body> for the same reason as the player — see the note there.
  return createPortal(
    <div
      className={cn(
        "fixed inset-0 z-50 bg-black/80 backdrop-blur-xl",
        leaving ? "cb-backdrop-out" : "cb-backdrop-in",
      )}
      onClick={cropping ? undefined : close}
    >
      <div
        className={cn(
          "flex h-full flex-col",
          leaving ? (shrinking ? "cb-chrome-out" : "cb-player-out") : "cb-player-in",
        )}
      >
        <header className="flex shrink-0 items-center justify-between gap-6 px-8 pt-6 pb-4">
          <div
            className="flex min-w-0 items-center gap-2"
            onClick={(event) => event.stopPropagation()}
          >
            <p className="truncate text-xs text-ink-muted">
              {clip.game ?? "Unknown game"} · {formatAgo(clip.createdAt)} ·{" "}
              {formatSize(clip.sizeBytes)}
            </p>
            <Pill className="bg-white/10">
              {clip.width} × {clip.height}
            </Pill>
            {hasOriginal && (
              <Pill className="bg-white/10 text-ink-muted" title="Cropped — the original sits beside it">
                Cropped
              </Pill>
            )}
          </div>
          <button
            aria-label="Close viewer"
            onClick={close}
            className="grid h-9 w-9 shrink-0 place-items-center rounded-pill border border-line
              text-ink-muted transition-colors hover:bg-elevated hover:text-ink"
          >
            <svg viewBox="0 0 14 14" className="h-3.5 w-3.5" stroke="currentColor" strokeWidth="1.4">
              <path d="M3.5 3.5l7 7M10.5 3.5l-7 7" />
            </svg>
          </button>
        </header>

        <div
          className="flex min-h-0 flex-1 gap-4 px-8"
          onClick={(event) => event.stopPropagation()}
        >
          <div
            ref={frame}
            onContextMenu={(event) =>
              !cropping && clipMenu(event, clip, { onDelete: () => setAskingDelete(true) })
            }
            className="relative min-h-0 min-w-0 flex-1 overflow-hidden rounded-card bg-black"
          >
            {broken || !source ? (
              <div className="grid h-full place-items-center px-8 text-center">
                <div>
                  <p className="text-sm font-medium">File not found</p>
                  <p className="mx-auto mt-2 max-w-md text-xs text-ink-muted">
                    {inTauri
                      ? `The file at ${clip.path} cannot be opened — it was probably moved or deleted outside of ClippiBoy.`
                      : "There are no real screenshots in browser mode."}
                  </p>
                </div>
              </div>
            ) : (
              <>
                <img
                  src={source}
                  alt={clip.title ?? "Screenshot"}
                  onError={() => setBroken(true)}
                  className="h-full w-full bg-black object-contain"
                />
                {cropping && (
                  <CropLayer
                    stage={frame}
                    width={clip.width}
                    height={clip.height}
                    ratio={ratioOf(ratio, clip.width, clip.height)}
                    rect={sel}
                    onRect={setSel}
                  />
                )}
              </>
            )}
          </div>

          <aside
            className="flex w-[380px] shrink-0 flex-col gap-6 overflow-y-auto rounded-card
              border border-line bg-surface/80 p-5 backdrop-blur-xl"
          >
            <ClipMeta clip={clip} />

            <section className="space-y-3">
              <h3 className="text-xs font-medium tracking-wide text-ink-faint uppercase">
                Crop
              </h3>
              {/* The presets are the way in: clicking one starts the crop with
                  that shape, rather than a button that only unlocks another
                  button. */}
              <div className="grid grid-cols-3 gap-1.5">
                {PRESETS.map((preset) => (
                  <button
                    key={preset}
                    type="button"
                    disabled={!inTauri || broken}
                    aria-pressed={cropping && ratio === preset}
                    onClick={() => pickPreset(preset)}
                    className={cn(
                      "h-8 rounded-inner border text-[13px] font-medium tabular-nums",
                      "transition-colors duration-150 ease-[var(--ease-out-soft)]",
                      "disabled:pointer-events-none disabled:opacity-40",
                      cropping && ratio === preset
                        ? "border-accent-bright bg-accent/20 text-ink"
                        : "border-line bg-elevated text-ink-muted hover:border-line-strong hover:text-ink",
                    )}
                  >
                    {preset}
                  </button>
                ))}
              </div>

              {cropping ? (
                <>
                  <p className="text-xs text-ink-muted tabular-nums">
                    {sel
                      ? `${Math.round(sel.width)} × ${Math.round(sel.height)} px`
                      : "Nothing selected"}
                    {sel && whole && " · the whole picture"}
                  </p>
                  <div className="flex gap-2">
                    <Button
                      size="sm"
                      variant="primary"
                      disabled={busy || whole}
                      onClick={applyCrop}
                    >
                      {busy ? "Cropping …" : "Crop"}
                    </Button>
                    <Button size="sm" variant="ghost" onClick={cancelCrop}>
                      Cancel
                    </Button>
                  </div>
                  <p className="text-xs text-ink-faint">
                    Drag on the picture to select, drag inside it to move it.
                    Escape leaves the crop.
                  </p>
                </>
              ) : hasOriginal ? (
                <>
                  <Button size="sm" variant="ghost" disabled={busy} onClick={undoCrop}>
                    {busy ? "Restoring …" : "Undo crop"}
                  </Button>
                  <p className="text-xs text-ink-faint">
                    The untouched picture sits beside it and comes back at any
                    time.
                  </p>
                </>
              ) : (
                <p className="text-xs text-ink-faint">
                  Cropping rewrites the file. The untouched picture is kept, so
                  it can be undone.
                </p>
              )}
            </section>
          </aside>
        </div>

        <footer
          className="flex shrink-0 items-center gap-2 px-8 pt-4 pb-6"
          onClick={(event) => event.stopPropagation()}
        >
          {/* The file only moves into the favorites folder once the viewer is
              closed — the gallery takes care of that. */}
          <Button
            size="sm"
            variant={clip.favorite ? "primary" : "secondary"}
            aria-pressed={clip.favorite}
            icon={
              <HeartBurst
                favorite={clip.favorite}
                className={cn(!clip.favorite && "text-live")}
              />
            }
            onClick={() => void setFavorite(clip.id, !clip.favorite)}
          >
            {clip.favorite ? "Favorite" : "Add to favorites"}
          </Button>
          <Button
            size="sm"
            variant="secondary"
            onClick={() => inTauri && api.revealClip(clip.id)}
          >
            Show in folder
          </Button>
          {askingDelete ? (
            <ConfirmDelete
              question="Delete screenshot?"
              onConfirm={removeClip}
              onCancel={() => setAskingDelete(false)}
            />
          ) : (
            <Button
              size="sm"
              variant="danger"
              icon={<IconTrash className="h-4 w-4" />}
              onClick={() => setAskingDelete(true)}
            >
              Delete
            </Button>
          )}

          <span className="ml-auto text-xs text-ink-faint">←/→ next picture</span>

          <div className="flex items-center gap-1">
            <Step label="Previous" disabled={index === 0} onClick={() => step(-1)}>
              ‹
            </Step>
            <span className="w-16 text-center text-xs text-ink-muted tabular-nums">
              {index + 1} / {clips.length}
            </span>
            <Step
              label="Next"
              disabled={index >= clips.length - 1}
              onClick={() => step(1)}
            >
              ›
            </Step>
          </div>
        </footer>
      </div>
    </div>,
    document.body,
  );
}

/**
 * The selection over the picture.
 *
 * Everything here is reckoned in pixels of the picture, and only turned into
 * percentages when it is drawn. That way the selection survives a resized window
 * and means the same thing on a 4K still as on a 1080p one — and the core gets
 * the rectangle in the only unit it can use.
 */
function CropLayer({
  stage,
  width,
  height,
  ratio,
  rect,
  onRect,
}: {
  stage: React.RefObject<HTMLDivElement | null>;
  width: number;
  height: number;
  ratio: number | null;
  rect: Rect | null;
  onRect: (rect: Rect) => void;
}) {
  /** Where the picture really sits in the stage — `object-contain` letterboxes. */
  const [box, setBox] = useState<{
    left: number;
    top: number;
    width: number;
    height: number;
    scale: number;
  } | null>(null);
  /** What the pointer is doing right now. `null` means: nothing. */
  const drag = useRef<
    | { kind: "draw"; anchorX: number; anchorY: number }
    | { kind: "move"; grabX: number; grabY: number }
    | null
  >(null);

  useLayoutEffect(() => {
    const el = stage.current;
    if (!el) return;
    const measure = () => {
      const rect = el.getBoundingClientRect();
      const scale = Math.min(rect.width / width, rect.height / height);
      setBox({
        left: (rect.width - width * scale) / 2,
        top: (rect.height - height * scale) / 2,
        width: width * scale,
        height: height * scale,
        scale,
      });
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [stage, width, height]);

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
    // A click without a drag means "start over", not a rectangle of nothing.
    if (rect && (rect.width < MIN_EDGE || rect.height < MIN_EDGE)) {
      onRect({ x: 0, y: 0, width, height });
    }
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
      className="absolute cursor-crosshair touch-none"
      style={{ left: box.left, top: box.top, width: box.width, height: box.height }}
      onPointerDown={(event) => onDown(event, "draw")}
      onPointerMove={onMove}
      onPointerUp={onUp}
    >
      {rect && rect.width > 0 && rect.height > 0 && (
        <div
          className="absolute cursor-move"
          style={{
            left: percent(rect.x, width),
            top: percent(rect.y, height),
            width: percent(rect.width, width),
            height: percent(rect.height, height),
            // Everything outside the selection goes dark. One shadow instead of
            // four panels — it can never leave a seam.
            boxShadow: "0 0 0 9999px rgba(0, 0, 0, 0.62)",
            outline: "1px solid rgba(255, 255, 255, 0.9)",
          }}
          onPointerDown={(event) => onDown(event, "move")}
          onPointerMove={onMove}
          onPointerUp={onUp}
        >
          {/* Thirds. They are the reason to crop by eye rather than by number. */}
          <div className="pointer-events-none absolute inset-0 opacity-40">
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

function Step({
  label,
  disabled,
  onClick,
  children,
}: {
  label: string;
  disabled: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      aria-label={label}
      disabled={disabled}
      onClick={onClick}
      className="grid h-8 w-8 place-items-center rounded-pill border border-line text-lg
        text-ink-muted transition-colors hover:bg-elevated hover:text-ink
        disabled:pointer-events-none disabled:opacity-30"
    >
      {children}
    </button>
  );
}
