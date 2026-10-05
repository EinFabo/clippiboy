import {
  createContext,
  useContext,
  useLayoutEffect,
  useMemo,
  useState,
  type RefObject,
} from "react";

/** Where a picture really sits inside its stage, and how far it is scaled. */
export interface PictureBox {
  left: number;
  top: number;
  width: number;
  height: number;
  /** Displayed pixels per picture pixel. */
  scale: number;
}

/**
 * How far the stage is zoomed in, and how far the picture is pushed aside.
 *
 * `zoom` multiplies the fitting scale (1 = the whole picture fits), `x`/`y`
 * move the picture away from the centre, in stage pixels.
 */
export interface PictureView {
  zoom: number;
  x: number;
  y: number;
}

export const WHOLE: PictureView = { zoom: 1, x: 0, y: 0 };

/**
 * The view every layer over the picture shares. A context rather than a prop
 * through each layer: they all measure their box through `usePictureBox`, and
 * this way a zoom reaches the crop handles and every mark without any of them
 * knowing about it.
 */
export const PictureViewContext = createContext<PictureView>(WHOLE);

/** The box for a stage of this size — see `usePictureBox`. */
export function boxFor(
  stage: { width: number; height: number },
  width: number,
  height: number,
  view: PictureView,
): PictureBox {
  const scale = Math.min(stage.width / width, stage.height / height) * view.zoom;
  return {
    left: (stage.width - width * scale) / 2 + view.x,
    top: (stage.height - height * scale) / 2 + view.y,
    width: width * scale,
    height: height * scale,
    scale,
  };
}

/**
 * Keep the picture on the stage: zoomed in, no edge may come away from the
 * stage's; smaller than the stage in a direction, it stays centred there.
 */
export function clampView(
  stage: { width: number; height: number },
  width: number,
  height: number,
  view: PictureView,
): PictureView {
  const zoom = Math.min(Math.max(view.zoom, 1), MAX_ZOOM);
  const scale = Math.min(stage.width / width, stage.height / height) * zoom;
  const room = (stageSize: number, size: number, shift: number) => {
    const slack = (size * scale - stageSize) / 2;
    return slack <= 0 ? 0 : Math.min(Math.max(shift, -slack), slack);
  };
  return { zoom, x: room(stage.width, width, view.x), y: room(stage.height, height, view.y) };
}

export const MAX_ZOOM = 8;

/**
 * Measure the letterbox.
 *
 * A picture shown with `object-contain` fills its stage in one direction only;
 * everything that lies over it — a crop selection, the annotations — has to know
 * where the other direction begins. Reckoning happens in pixels of the picture
 * throughout, and this is the one place that converts. The zoom from
 * `PictureViewContext` is part of it.
 */
export function usePictureBox(
  stage: RefObject<HTMLElement | null>,
  width: number,
  height: number,
): PictureBox | null {
  const size = useStageSize(stage);
  const view = useContext(PictureViewContext);
  return useMemo(
    () => (size ? boxFor(size, width, height, view) : null),
    [size, width, height, view],
  );
}

/** The stage's own size, kept up to date. */
export function useStageSize(
  stage: RefObject<HTMLElement | null>,
): { width: number; height: number } | null {
  const [size, setSize] = useState<{ width: number; height: number } | null>(null);

  useLayoutEffect(() => {
    const element = stage.current;
    if (!element) return;
    const measure = () => {
      // The layout size, not the bounding box: the viewer scales in as it
      // opens, and a box measured mid-animation would stay too small — a
      // resize observer does not hear transforms end.
      const rect = { width: element.clientWidth, height: element.clientHeight };
      setSize((was) =>
        was && was.width === rect.width && was.height === rect.height
          ? was
          : { width: rect.width, height: rect.height },
      );
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [stage]);

  return size;
}
