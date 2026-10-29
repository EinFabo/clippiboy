import { useEffect, useState, type RefObject } from "react";
import { camBox } from "@/lib/facecam";
import { fileUrl } from "@/lib/ipc";
import type { PictureBox } from "@/lib/pictureBox";
import type { CamLayout, FacecamView } from "@/lib/types";
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
}) {
  const [clean, setClean] = useState<HTMLVideoElement | null>(null);
  const [cam, setCam] = useState<HTMLVideoElement | null>(null);
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
