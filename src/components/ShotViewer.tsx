import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { createPortal } from "react-dom";

import { Button } from "@/components/ui/Button";
import { Pill } from "@/components/ui/Card";
import { Segmented, Slider } from "@/components/ui/Controls";
import { ConfirmDelete } from "@/components/ui/ConfirmDelete";
import { HeartBurst } from "@/components/ui/HeartBurst";
import { ClipMeta } from "@/components/ClipMeta";
import { ColorPicker } from "@/components/ui/ColorPicker";
import {
  AnnotateLayer,
  COLORS,
  TOOLS,
  BLUR_SHAPES,
  LOOKS,
  hasAltColor,
  hasColor,
  hasLook,
  hasShape,
  lookOf,
  paint,
  range,
  rimColor,
  toSteps,
  sizeLabel,
  type Shape,
  type Style,
  type Tool,
} from "@/components/ShotAnnotate";
import { useClipMenu } from "@/components/clipMenu";
import { IconTrash } from "@/components/icons";
import { api, fileUrl, inTauri } from "@/lib/ipc";
import { formatAgo, formatSize } from "@/lib/format";
import { EASE_EXIT, prefersReducedMotion } from "@/lib/motion";
import { cn } from "@/lib/cn";
import { usePictureBox } from "@/lib/pictureBox";
import { useEngine } from "@/store";
import type { Clip, ShotEdit } from "@/lib/types";

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
  const writeScreenshot = useEngine((state) => state.writeScreenshot);
  const clipMenu = useClipMenu();

  const [leaving, setLeaving] = useState(false);
  const [shrinking, setShrinking] = useState(false);
  const [askingDelete, setAskingDelete] = useState(false);
  const [broken, setBroken] = useState(false);
  /** Looking, cropping or drawing — never two of them at once. */
  const [mode, setMode] = useState<"view" | "crop" | "draw">("view");
  const [ratio, setRatio] = useState<string>("Free");
  const [sel, setSel] = useState<Rect | null>(null);
  const [busy, setBusy] = useState(false);
  /**
   * What has been done to this picture so far, as the core keeps it.
   *
   * Nothing is ever flattened into the file for good: crop and marks are held
   * apart and the picture is rebuilt from the untouched original on every save.
   * Only that lets either of them be taken back without the other.
   */
  const [edit, setEdit] = useState<ShotEdit | null>(null);
  const [tool, setTool] = useState<Tool>("arrow");
  /** Every tool keeps its own settings — a red arrow does not make the text red. */
  const [styles, setStyles] = useState<Record<string, Style>>(() =>
    defaultStyles(clips[index]?.width ?? 1920),
  );
  const [shapes, setShapes] = useState<Shape[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [open, setOpen] = useState({ annotate: true, crop: false });

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
  const pictureWidth = clip?.width ?? 1920;

  // Everything about the previous picture says nothing about this one.
  useEffect(() => {
    setBroken(false);
    setMode("view");
    setSel(null);
    setShapes([]);
    setSelected(null);
    setEdit(null);
    // The defaults are reckoned from the picture: a stroke that reads well on
    // 4K is a bar on 1080p.
    setStyles(defaultStyles(pictureWidth));
    if (!id || !inTauri) return;
    let current = true;
    api
      .screenshotEdit(id)
      .then((loaded) => {
        if (!current) return;
        setEdit(loaded);
        setShapes(readMarks(loaded));
      })
      .catch(() => {});
    return () => {
      current = false;
    };
  }, [id, pictureWidth]);

  // A picture that would not load says nothing about the next one — and the
  // stage shows a different file in each mode: the finished one while looking,
  // the ground without the marks while drawing, the original while cropping.
  useEffect(() => setBroken(false), [mode, id, edit?.basePath, edit?.originalPath]);

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
        if (mode !== "view") leaveEditing();
        else close();
      } else if (
        mode === "draw" &&
        selected !== null &&
        (event.key === "Delete" || event.key === "Backspace")
      ) {
        event.preventDefault();
        dropSelected();
      } else if (mode === "draw" && (event.ctrlKey || event.metaKey) && event.key === "z") {
        // The last mark back, one at a time. Nothing to redo: whoever wants it
        // again draws it again — that is quicker than finding the button.
        event.preventDefault();
        setShapes((drawn) => drawn.slice(0, -1));
      } else if (mode === "view" && event.key === "ArrowLeft") {
        step(-1);
      } else if (mode === "view" && event.key === "ArrowRight") {
        step(1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // `dropSelected` is stable enough for this: it only reads state setters.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [askingDelete, close, mode, selected, step]);

  if (!clip) return null;



  /** The edges every mark and every crop is reckoned in. */
  const full = {
    width: edit?.originalWidth ?? clip.width,
    height: edit?.originalHeight ?? clip.height,
  };
  /** Where the picture being drawn on sits inside the original. */
  const origin = edit?.crop ?? { x: 0, y: 0 };
  /**
   * The picture on the stage, and what is shown of it — see `ShotEdit`.
   *
   * The cache buster is the file size: after an edit there is a new picture
   * under the same path, and the WebView would otherwise keep showing the old
   * one out of its cache.
   */
  const ground =
    mode === "crop"
      ? { src: edit?.originalPath ?? clip.path, ...full }
      : mode === "draw"
        ? {
            src: edit?.basePath ?? clip.path,
            width: edit?.crop?.width ?? full.width,
            height: edit?.crop?.height ?? full.height,
          }
        : { src: clip.path, width: clip.width, height: clip.height };
  const source = inTauri ? `${fileUrl(ground.src)}?v=${clip.sizeBytes}` : null;

  const edited = Boolean(edit?.crop) || shapes.length > 0;

  /**
   * A preset both starts the crop and shapes it.
   *
   * From standing still that means one click for the whole job: the selection
   * begins as whatever is cropped right now — or the whole picture — and the
   * ratio trims it to shape around its centre.
   */
  const pickPreset = (preset: string) => {
    const base: Rect =
      (mode === "crop" ? sel : null) ?? edit?.crop ?? { x: 0, y: 0, ...full };
    const wanted = ratioOf(preset, full.width, full.height);
    setRatio(preset);
    setSel(wanted ? fitRatio(base, wanted, full.width, full.height) : base);
    setMode("crop");
    setSelected(null);
  };

  /** Back to plain looking. Unsaved marks go, saved ones stay saved. */
  const leaveEditing = () => {
    setMode("view");
    setSel(null);
    setSelected(null);
    if (edit) setShapes(readMarks(edit));
  };

  const pickTool = (next: Tool) => {
    setTool(next);
    setMode("draw");
    setSel(null);
    // A drawing tool means: the next thing is new. Keeping the old selection
    // would leave its handles lying over the picture you are drawing on.
    if (next !== "select") setSelected(null);
  };

  /**
   * Write the picture: original, then marks, then crop.
   *
   * Both halves of the editor come through here, and that is the point — the
   * core rebuilds from the untouched picture every time, so the crop can go
   * without taking the marks with it and the other way round.
   *
   * The marks travel as layers rendered at the **original's** size: they are
   * kept in its coordinates, so a changed crop leaves them where they were.
   */
  const write = async (crop: Rect | null, marks: Shape[]) => {
    setBusy(true);
    try {
      // From the ground they were drawn on into the original's coordinates.
      const placed = shift(marks, origin.x, origin.y);
      const steps = await Promise.all(
        toSteps(placed).map(async (step) =>
          step.kind === "blur"
            ? step
            : {
                kind: "layer" as const,
                png: await layerBytes(step.shapes, full.width, full.height),
              },
        ),
      );
      await writeScreenshot(
        clip.id,
        crop && {
          x: Math.round(crop.x),
          y: Math.round(crop.y),
          width: Math.round(crop.width),
          height: Math.round(crop.height),
        },
        steps,
        placed.length ? JSON.stringify(placed) : "",
      );
      // Read back rather than guessed: the crop may have moved the marks into
      // another frame of reference, and the core is the one that knows.
      const loaded = await api.screenshotEdit(clip.id);
      setEdit(loaded);
      setShapes(readMarks(loaded));
      setSelected(null);
      setSel(null);
      setMode("view");
    } catch {
      // The notice is already in the store.
    } finally {
      setBusy(false);
    }
  };

  /**
   * What the options below act on: the selected mark if there is one, otherwise
   * what the next mark will be. One panel for both — a separate "properties"
   * pane would say the same things twice.
   */
  const chosen = shapes.find((shape) => shape.id === selected) ?? null;
  const active: Tool = chosen?.tool ?? tool;
  const style: Style = chosen
    ? {
        color: chosen.color,
        altColor:
          chosen.altColor ??
          (chosen.tool === "text" ? rimColor(chosen.color) : chosen.color),
        size: chosen.size,
        fill: chosen.fill ?? false,
        outline: chosen.outline !== false,
        round: chosen.round ?? false,
      }
    : (styles[tool] ?? defaultStyles(full.width).arrow);

  const restyle = (patch: Partial<Style>) => {
    if (chosen) {
      setShapes((drawn) =>
        drawn.map((shape) => (shape.id === chosen.id ? { ...shape, ...patch } : shape)),
      );
      return;
    }
    setStyles((all) => ({ ...all, [tool]: { ...style, ...patch } }));
  };

  const sizeRange = range(active, full.width);

  const dropSelected = () => {
    if (selected === null) return;
    setShapes((drawn) => drawn.filter((shape) => shape.id !== selected));
    setSelected(null);
  };

  const whole =
    !sel ||
    (sel.x === 0 &&
      sel.y === 0 &&
      Math.round(sel.width) >= full.width &&
      Math.round(sel.height) >= full.height);

  // Hung under <body> for the same reason as the player — see the note there.
  return createPortal(
    <div
      className={cn(
        "fixed inset-0 z-50 bg-black/80 backdrop-blur-xl",
        leaving ? "cb-backdrop-out" : "cb-backdrop-in",
      )}
      onClick={mode === "view" ? close : undefined}
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
            {edited && (
              <Pill
                className="bg-white/10 text-ink-muted"
                title="Edited — the untouched picture sits beside it"
              >
                Edited
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
              mode === "view" &&
              clipMenu(event, clip, { onDelete: () => setAskingDelete(true) })
            }
            className="relative min-h-0 min-w-0 flex-1 overflow-hidden rounded-card bg-black"
          >
            {broken || !source ? (
              <div className="grid h-full place-items-center px-8 text-center">
                <div>
                  <p className="text-sm font-medium">File not found</p>
                  <p className="mx-auto mt-2 max-w-md text-xs text-ink-muted">
                    {inTauri
                      ? `The file at ${ground.src} cannot be opened — it was probably moved or deleted outside of ClippiBoy.`
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
                {mode === "draw" && (
                  <AnnotateLayer
                    stage={frame}
                    width={ground.width}
                    height={ground.height}
                    tool={tool}
                    style={style}
                    shapes={shapes}
                    onShapes={setShapes}
                    selected={selected}
                    onSelect={setSelected}
                  />
                )}
                {mode === "crop" && (
                  <CropLayer
                    stage={frame}
                    width={full.width}
                    height={full.height}
                    ratio={ratioOf(ratio, full.width, full.height)}
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

            <Disclosure
              title="Annotate"
              open={open.annotate}
              onToggle={() => setOpen((was) => ({ ...was, annotate: !was.annotate }))}
              badge={shapes.length > 0 ? String(shapes.length) : undefined}
            >
              <div className="grid grid-cols-4 gap-1.5">
                {TOOLS.map(([key, label]) => (
                  <PanelButton
                    key={key}
                    active={mode === "draw" && tool === key}
                    disabled={!inTauri || broken}
                    onClick={() => pickTool(key)}
                  >
                    {label}
                  </PanelButton>
                ))}
              </div>

              {/* Only what this tool actually has. A colour for the blur or a
                  fill for an arrow would be a control that does nothing. */}
              {hasColor(active) && (
                <div className="space-y-1.5">
                  <span className="text-xs text-ink-faint">
                    {active === "text" ? "Text" : "Colour"}
                  </span>
                  <ColorPicker
                    value={style.color}
                    swatches={COLORS}
                    onChange={(color) => restyle({ color })}
                  />
                </div>
              )}

              {hasAltColor(active, style) && (
                <div className="space-y-1.5">
                  <span className="text-xs text-ink-faint">
                    {active === "text" ? "Rim" : "Border"}
                  </span>
                  <ColorPicker
                    value={style.altColor}
                    swatches={COLORS}
                    onChange={(altColor) => restyle({ altColor })}
                  />
                </div>
              )}

              <div className="flex items-center gap-2">
                <span className="w-16 shrink-0 text-xs text-ink-faint">
                  {sizeLabel(active)}
                </span>
                <Slider
                  label={sizeLabel(active)}
                  value={style.size}
                  min={sizeRange[0]}
                  max={sizeRange[1]}
                  onChange={(size) => restyle({ size })}
                />
                <span className="w-8 shrink-0 text-right text-xs text-ink-muted tabular-nums">
                  {Math.round(style.size)}
                </span>
              </div>

              {hasLook(active) && (
                <div className="flex items-center gap-2">
                  <span className="w-16 shrink-0 text-xs text-ink-faint">Look</span>
                  <Segmented
                    value={lookOf(style)}
                    options={LOOKS.map(([label]) => ({ key: label, label }))}
                    onChange={(next) => {
                      const look = LOOKS.find(([label]) => label === next)?.[1];
                      if (look) restyle(look);
                    }}
                  />
                </div>
              )}

              {hasShape(active) && (
                <div className="flex items-center gap-2">
                  <span className="w-16 shrink-0 text-xs text-ink-faint">Shape</span>
                  <Segmented
                    value={style.round ? "Ellipse" : "Rectangle"}
                    options={BLUR_SHAPES.map(([label]) => ({ key: label, label }))}
                    onChange={(next) =>
                      restyle({
                        round: BLUR_SHAPES.find(([label]) => label === next)?.[1] ?? false,
                      })
                    }
                  />
                </div>
              )}

              {chosen && (
                <div className="flex items-center gap-2">
                  <span className="text-xs text-ink-muted">
                    {TOOLS.find(([key]) => key === chosen.tool)?.[1]} selected
                  </span>
                  <Button
                    size="sm"
                    variant="ghost"
                    className="ml-auto"
                    onClick={dropSelected}
                  >
                    Remove
                  </Button>
                </div>
              )}

              {mode === "draw" && (
                <>
                  <div className="flex gap-2">
                    <Button
                      size="sm"
                      variant="primary"
                      disabled={busy || shapes.length === 0}
                      onClick={() => write(edit?.crop ?? null, shapes)}
                    >
                      {busy ? "Saving …" : "Save marks"}
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={shapes.length === 0}
                      onClick={() => {
                        setShapes((drawn) => drawn.slice(0, -1));
                        setSelected(null);
                      }}
                    >
                      Undo
                    </Button>
                    <Button size="sm" variant="ghost" onClick={leaveEditing}>
                      Discard
                    </Button>
                  </div>
                  {/* Only the marks go — the crop is kept apart from them. */}
                  {edit?.marks && (
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={busy}
                      onClick={() => write(edit?.crop ?? null, [])}
                    >
                      Remove all marks
                    </Button>
                  )}
                  <p className="text-xs text-ink-faint">
                    {tool === "blur"
                      ? "Drag over what should not be readable. What you see here is what lands in the file."
                      : tool === "text"
                        ? "Click and write — at the size and in the colour it will have. Enter makes a line, Escape is done, an empty text removes it again."
                        : "A finished mark stays selected — drag it, or pull its corners. Select picks up an older one, double-click a text to rewrite it. Nothing is written until you save."}
                  </p>
                </>
              )}
            </Disclosure>

            <Disclosure
              title="Crop"
              open={open.crop}
              onToggle={() => setOpen((was) => ({ ...was, crop: !was.crop }))}
            >
              {/* The presets are the way in: clicking one starts the crop with
                  that shape, rather than a button that only unlocks another
                  button. */}
              <div className="grid grid-cols-3 gap-1.5">
                {PRESETS.map((preset) => (
                  <PanelButton
                    key={preset}
                    active={mode === "crop" && ratio === preset}
                    disabled={!inTauri || broken}
                    onClick={() => pickPreset(preset)}
                  >
                    {preset}
                  </PanelButton>
                ))}
              </div>

              {mode === "crop" ? (
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
                      onClick={() => sel && write(sel, shapes)}
                    >
                      {busy ? "Cropping …" : "Crop"}
                    </Button>
                    <Button size="sm" variant="ghost" onClick={leaveEditing}>
                      Cancel
                    </Button>
                  </div>
                  <p className="text-xs text-ink-faint">
                    Drag on the picture to select, drag inside it to move it.
                    Escape leaves the crop.
                  </p>
                </>
              ) : edit?.crop ? (
                <>
                  <p className="text-xs text-ink-muted tabular-nums">
                    Cropped to {edit.crop.width} × {edit.crop.height} px
                  </p>
                  {/* Only the crop goes — the marks are kept apart from it and
                      are drawn again onto the whole picture. */}
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={busy}
                    onClick={() => write(null, shapes)}
                  >
                    {busy ? "Restoring …" : "Undo crop"}
                  </Button>
                </>
              ) : (
                <p className="text-xs text-ink-faint">
                  The untouched picture is kept beside it, so a crop can be
                  taken back at any time — the marks stay where they are.
                </p>
              )}
            </Disclosure>
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

/**
 * The saved marks, moved into the coordinates of the picture they are shown on.
 *
 * They are kept in the original's, so a changed crop leaves them where they
 * were — what shifts is only the frame they are looked at through.
 */
function readMarks(edit: ShotEdit): Shape[] {
  if (!edit.marks) return [];
  try {
    const marks = JSON.parse(edit.marks) as Shape[];
    return shift(marks, -(edit.crop?.x ?? 0), -(edit.crop?.y ?? 0));
  } catch {
    // A note we cannot read is not worth losing the picture over.
    return [];
  }
}

function shift(shapes: Shape[], dx: number, dy: number): Shape[] {
  if (dx === 0 && dy === 0) return shapes;
  return shapes.map((shape) => ({
    ...shape,
    x1: shape.x1 + dx,
    y1: shape.y1 + dy,
    x2: shape.x2 + dx,
    y2: shape.y2 + dy,
    points: shape.points?.map(([x, y]) => [x + dx, y + dy] as [number, number]),
  }));
}

/** What every tool starts out as on a picture of this width. */
function defaultStyles(width: number): Record<string, Style> {
  const entry = (tool: Tool): Style => ({
    color: COLORS[0],
    // Text starts with the rim that keeps it readable; a border starts as the
    // same ink as the fill, which is the shape you drew before there was a
    // choice at all.
    altColor: tool === "text" ? rimColor(COLORS[0]) : COLORS[0],
    size: range(tool, width)[2],
    fill: false,
    outline: true,
    round: false,
  });
  return {
    arrow: entry("arrow"),
    box: entry("box"),
    ellipse: entry("ellipse"),
    pen: entry("pen"),
    text: entry("text"),
    blur: entry("blur"),
    select: entry("arrow"),
  };
}

/**
 * A section of the side column that can be folded away.
 *
 * Two editors under one another are a lot at once, and most of the time you
 * only want one of them — so whichever you are not using gets out of the way.
 */
function Disclosure({
  title,
  badge,
  open,
  onToggle,
  children,
}: {
  title: string;
  badge?: string;
  open: boolean;
  onToggle: () => void;
  children: React.ReactNode;
}) {
  return (
    <section>
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        className="flex w-full items-center gap-2 py-1 text-xs font-medium tracking-wide
          text-ink-faint uppercase transition-colors hover:text-ink-muted"
      >
        <svg
          viewBox="0 0 24 24"
          className={cn(
            "h-3.5 w-3.5 transition-transform duration-200 ease-[var(--ease-out-soft)]",
            open && "rotate-90",
          )}
          fill="none"
          stroke="currentColor"
          strokeWidth="2.4"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <path d="M9 6l6 6-6 6" />
        </svg>
        {title}
        {badge && (
          <span className="rounded-pill bg-accent/25 px-2 py-0.5 text-[10px] text-accent-bright">
            {badge}
          </span>
        )}
      </button>
      {open && <div className="mt-3 space-y-3">{children}</div>}
    </section>
  );
}

/** A small pressable in the side column — presets, tools, sizes. */
function PanelButton({
  active,
  disabled,
  onClick,
  className,
  children,
}: {
  active: boolean;
  disabled?: boolean;
  onClick: () => void;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      aria-pressed={active}
      onClick={onClick}
      className={cn(
        "h-8 rounded-inner border text-[13px] font-medium tabular-nums",
        "transition-colors duration-150 ease-[var(--ease-out-soft)]",
        "disabled:pointer-events-none disabled:opacity-40",
        active
          ? "border-accent-bright bg-accent/20 text-ink"
          : "border-line bg-elevated text-ink-muted hover:border-line-strong hover:text-ink",
        className,
      )}
    >
      {children}
    </button>
  );
}

/**
 * A run of marks as PNG bytes.
 *
 * Rendered afresh rather than read off the preview: both go through the same
 * `paint`, so there is nothing that could drift apart, and the preview keeps
 * its canvases to itself.
 */
async function layerBytes(
  shapes: Shape[],
  width: number,
  height: number,
): Promise<number[]> {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("The marks could not be turned into a picture.");
  paint(ctx, shapes);

  const blob = await new Promise<Blob | null>((resolve) =>
    canvas.toBlob(resolve, "image/png"),
  );
  if (!blob) throw new Error("The marks could not be turned into a picture.");
  return Array.from(new Uint8Array(await blob.arrayBuffer()));
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
