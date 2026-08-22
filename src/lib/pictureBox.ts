import { useLayoutEffect, useState, type RefObject } from "react";

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
 * Measure the letterbox.
 *
 * A picture shown with `object-contain` fills its stage in one direction only;
 * everything that lies over it — a crop selection, the annotations — has to know
 * where the other direction begins. Reckoning happens in pixels of the picture
 * throughout, and this is the one place that converts.
 */
export function usePictureBox(
  stage: RefObject<HTMLElement | null>,
  width: number,
  height: number,
): PictureBox | null {
  const [box, setBox] = useState<PictureBox | null>(null);

  useLayoutEffect(() => {
    const element = stage.current;
    if (!element) return;
    const measure = () => {
      const rect = element.getBoundingClientRect();
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
    observer.observe(element);
    return () => observer.disconnect();
  }, [stage, width, height]);

  return box;
}
