// The facecam's shared helpers for the settings tab and the console.
import { api } from "./ipc";
import type { CamLayout, CamState, OverlayCorner } from "./types";
export { camBox } from "./camGeometry";

/** The corner a layout sits in, or `null` when it sits anywhere else. */
export function cornerOf(layout: CamLayout): OverlayCorner | null {
  const edge = (v: number) => (v === 0 ? 0 : v === 1 ? 1 : null);
  const x = edge(layout.x);
  const y = edge(layout.y);
  if (x === null || y === null) return null;
  return y === 0 ? (x === 0 ? "topLeft" : "topRight") : x === 0 ? "bottomLeft" : "bottomRight";
}

export function layoutAt(corner: OverlayCorner): { x: number; y: number } {
  return {
    x: corner === "topLeft" || corner === "bottomLeft" ? 0 : 1,
    y: corner === "topLeft" || corner === "topRight" ? 0 : 1,
  };
}

/** One line on what the camera is doing. */
export function camStateLine(state: CamState): string {
  switch (state.kind) {
    case "live":
      return "On";
    case "taken":
      return state.by ? `Taken by ${state.by}` : "Taken by another app";
    case "blocked":
      return "Blocked by Windows privacy";
    case "noCamera":
      return "No camera";
    case "starting":
      return "Starting…";
    case "failed":
      return state.message;
  }
}

/** A preview frame from the core: seq, width and height, then BGRA pixels. */
export function decodePreview(
  buffer: ArrayBuffer,
): { seq: number; width: number; height: number; pixels: Uint8ClampedArray<ArrayBuffer> } | null {
  if (buffer.byteLength < 16) return null;
  const view = new DataView(buffer);
  const seq = Number(view.getBigUint64(0, true));
  const width = view.getUint32(8, true);
  const height = view.getUint32(12, true);
  if (buffer.byteLength < 16 + width * height * 4) return null;
  const pixels = new Uint8ClampedArray(buffer, 16, width * height * 4);
  // BGRA from the core, RGBA for the canvas.
  for (let i = 0; i < width * height * 4; i += 4) {
    const b = pixels[i];
    pixels[i] = pixels[i + 2];
    pixels[i + 2] = b;
  }
  return { seq, width, height, pixels };
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Pulls preview frames into `canvas` until the returned function is called:
 * asks for the next frame after drawing one, waits 200 ms after an empty
 * answer, stops on a refusal. With `onlyWhileVisible` it pauses while the page
 * is hidden, so a window in the tray does not keep the camera on.
 */
export function startCamPreview(canvas: HTMLCanvasElement, opts: { onlyWhileVisible: boolean }): () => void {
  let running = true;
  void (async () => {
    let after = 0;
    while (running) {
      if (opts.onlyWhileVisible && document.visibilityState !== "visible") {
        await sleep(500);
        continue;
      }
      let buffer: ArrayBuffer;
      try {
        buffer = await api.camPreviewFrame(after);
      } catch {
        return;
      }
      if (!running) return;
      const frame = decodePreview(buffer);
      if (!frame) {
        await sleep(200);
        continue;
      }
      after = frame.seq;
      if (canvas.width !== frame.width) canvas.width = frame.width;
      if (canvas.height !== frame.height) canvas.height = frame.height;
      canvas.getContext("2d")?.putImageData(new ImageData(frame.pixels, frame.width, frame.height), 0, 0);
    }
  })();
  return () => {
    running = false;
  };
}
