import { useEffect, useState, type RefObject } from "react";
import { camBox } from "@/lib/facecam";
import { portraitBoxes } from "@/lib/camGeometry";
import { fileUrl } from "@/lib/ipc";
import type { PictureBox } from "@/lib/pictureBox";
import type { CamLayout, FacecamView, PortraitLayout } from "@/lib/types";

/** The 9:16 preview: the layout, and what dragging its handles does. */
export interface PortraitPreview {
  layout: PortraitLayout;
  onChange: (layout: PortraitLayout) => void;
  onCommit: (layout: PortraitLayout) => void;
}
import { inGap } from "@/lib/slaved";
import { useSlavedMedia } from "@/lib/useSlavedMedia";

/**
 * The clean picture with the camera over it, where the layout puts it, both
 * following the clip's `<video>` — whose own picture is hidden meanwhile, so a
 * file that already shows the camera never shows it twice. Clicks pass
 * through to the clip element.
 */
export function CamStage({
  view,
  layout,
  master,
  bindKey,
  box,
  frame,
  released,
  elements,
  portrait = null,
  stageSize = null,
}: {
  view: FacecamView;
  layout: CamLayout;
  master: RefObject<HTMLVideoElement | null>;
  bindKey: unknown;
  /** Where the clip's picture sits on the stage. */
  box: PictureBox | null;
  /** The clip's size in pixels. */
  frame: { width: number; height: number };
  /** A save or undo has the files: let go of them. */
  released: boolean;
  /** Filled with the two elements, for the frozen frame behind a save. */
  elements?: { current: { clean: HTMLVideoElement | null; cam: HTMLVideoElement | null } };
  /** Shows the 9:16 export instead, in a frame fitted to `stageSize`. */
  portrait?: PortraitPreview | null;
  stageSize?: { width: number; height: number } | null;
}) {
  const [clean, setClean] = useState<HTMLVideoElement | null>(null);
  const [cam, setCam] = useState<HTMLVideoElement | null>(null);
  if (elements) elements.current = { clean, cam };
  useSlavedMedia(
    master,
    [
      { element: clean, leadMs: view.cleanLeadMs },
      { element: layout.visible ? cam : null, leadMs: view.camLeadMs },
    ],
    bindKey,
  );

  // Where the camera was gone while recording, the clip shows none either.
  const [gap, setGap] = useState(false);
  useEffect(() => {
    const element = master.current;
    if (!element || view.gapsMs.length === 0) return;
    let handle = 0;
    let shown = false;
    const check = () => {
      const now = inGap(element.currentTime, view.cleanLeadMs, view.gapsMs);
      if (now !== shown) {
        shown = now;
        setGap(now);
      }
    };
    const onFrame = () => {
      check();
      handle = element.requestVideoFrameCallback(onFrame);
    };
    handle = element.requestVideoFrameCallback(onFrame);
    element.addEventListener("seeked", check);
    check();
    return () => {
      element.cancelVideoFrameCallback(handle);
      element.removeEventListener("seeked", check);
    };
  }, [master, view, bindKey]);

  // Windows will not move a file that is still open.
  useEffect(() => {
    if (!released) return;
    for (const element of [clean, cam]) {
      if (!element) continue;
      element.pause();
      element.removeAttribute("src");
      element.load();
    }
  }, [released, clean, cam]);

  if (portrait && stageSize) {
    return (
      <PortraitStage
        view={view}
        layout={layout}
        frame={frame}
        portrait={portrait}
        stage={stageSize}
        released={released}
        setClean={setClean}
        setCam={setCam}
        gap={gap}
      />
    );
  }
  if (!box) return null;
  const place = camBox(layout, frame.width, frame.height);
  const radius = layout.shape === "circle" ? "50%" : layout.shape === "rounded" ? `${0.12 * place.height * box.scale}px` : 0;
  return (
    <div className="pointer-events-none absolute inset-0">
      <video
        ref={setClean}
        src={released ? undefined : fileUrl(view.cleanPath)}
        muted
        playsInline
        preload="auto"
        className="absolute object-contain"
        style={{ left: box.left, top: box.top, width: box.width, height: box.height }}
      />
      {layout.visible && (
        <video
          ref={setCam}
          src={released ? undefined : fileUrl(view.camPath)}
          muted
          playsInline
          preload="auto"
          className="absolute object-cover"
          style={{
            left: box.left + place.left * box.scale,
            top: box.top + place.top * box.scale,
            width: place.width * box.scale,
            height: place.height * box.scale,
            borderRadius: radius,
            transform: layout.mirror ? "scaleX(-1)" : undefined,
            visibility: gap ? "hidden" : undefined,
          }}
        />
      )}
    </div>
  );
}

/**
 * The 9:16 export as it will come out: the camera fills the band on top, the
 * clean picture shows through the game window below. The same two elements as
 * the landscape stage, placed by CSS only — a `<video>` moved to another
 * parent reloads in WebView2. The numbers are `portraitBoxes`, the twin of the
 * filter's geometry, at 1080×1920 and scaled to the frame.
 */
function PortraitStage({
  view,
  layout,
  frame,
  portrait,
  stage,
  released,
  setClean,
  setCam,
  gap,
}: {
  view: FacecamView;
  layout: CamLayout;
  frame: { width: number; height: number };
  portrait: PortraitPreview;
  stage: { width: number; height: number };
  released: boolean;
  setClean: (element: HTMLVideoElement | null) => void;
  setCam: (element: HTMLVideoElement | null) => void;
  gap: boolean;
}) {
  const fh = Math.min(stage.height, (stage.width * 16) / 9);
  const fw = (fh * 9) / 16;
  const fx = (stage.width - fw) / 2;
  const fy = (stage.height - fh) / 2;
  const k = fw / 1080;
  const cam = layout.visible ? { width: view.camWidth, height: view.camHeight } : null;
  const g = portraitBoxes(frame, cam, portrait.layout, { width: 1080, height: 1920 });
  const c = g.gameCrop;
  const free = g.gameScaled.width - c.width;

  // Dragging: the split line moves the band's edge, the game moves its window.
  const drag = (kind: "split" | "game") => (down: React.PointerEvent) => {
    down.preventDefault();
    down.stopPropagation();
    const target = down.currentTarget as HTMLElement;
    target.setPointerCapture(down.pointerId);
    const start = { x: down.clientX, y: down.clientY, layout: portrait.layout };
    let last = portrait.layout;
    const move = (event: PointerEvent) => {
      const next =
        kind === "split"
          ? { ...start.layout, split: Math.min(0.5, Math.max(0.25, start.layout.split + (event.clientY - start.y) / fh)) }
          : {
              ...start.layout,
              // Dragging the picture right shows more of its left.
              gameX: free > 0 ? Math.min(1, Math.max(0, start.layout.gameX - (event.clientX - start.x) / (free * k))) : start.layout.gameX,
            };
      last = next;
      portrait.onChange(next);
    };
    const up = () => {
      target.removeEventListener("pointermove", move);
      target.removeEventListener("pointerup", up);
      portrait.onCommit(last);
    };
    target.addEventListener("pointermove", move);
    target.addEventListener("pointerup", up);
  };

  const inset = `inset(${c.y * k}px ${(g.gameScaled.width - c.x - c.width) * k}px ${(g.gameScaled.height - c.y - c.height) * k}px ${c.x * k}px)`;
  return (
    <div className="absolute inset-0 bg-black">
      <div className="absolute bg-elevated/40" style={{ left: fx, top: fy, width: fw, height: fh }} />
      <video
        ref={setClean}
        src={released ? undefined : fileUrl(view.cleanPath)}
        muted
        playsInline
        preload="auto"
        className="absolute object-fill"
        style={{
          left: fx + (g.gameAt.x - c.x) * k,
          top: fy + (g.gameAt.y - c.y) * k,
          width: g.gameScaled.width * k,
          height: g.gameScaled.height * k,
          clipPath: inset,
        }}
      />
      {g.band && (
        <video
          ref={setCam}
          src={released ? undefined : fileUrl(view.camPath)}
          muted
          playsInline
          preload="auto"
          className="absolute object-cover"
          style={{
            left: fx + g.band.x * k,
            top: fy + g.band.y * k,
            width: g.band.width * k,
            height: g.band.height * k,
            transform: layout.mirror ? "scaleX(-1)" : undefined,
            visibility: gap ? "hidden" : undefined,
          }}
        />
      )}
      {/* The game window drags sideways; the line between band and game sets the split. */}
      <div
        className="absolute cursor-ew-resize"
        style={{ left: fx, top: fy + g.gameAt.y * k, width: fw, height: c.height * k }}
        onPointerDown={drag("game")}
      />
      {g.band && (
        <div
          className="absolute flex cursor-ns-resize items-center justify-center"
          style={{ left: fx, top: fy + g.band.height * k - 6, width: fw, height: 12 }}
          onPointerDown={drag("split")}
        >
          <div className="h-0.5 w-full bg-white/70" />
        </div>
      )}
    </div>
  );
}
