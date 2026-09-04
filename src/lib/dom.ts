/** Text fields where a menu of our own with paste makes sense. */
const TEXT_TYPES = ["text", "search", "url", "email", "tel", "password", "number"];

export type TextField = HTMLInputElement | HTMLTextAreaElement;

/**
 * Is the target a field you can type into?
 *
 * Two places ask: the global text field menu, and the clip tile, which holds
 * back its own menu while the cursor is over its name field.
 */
export function isTextField(target: EventTarget | null): target is TextField {
  if (target instanceof HTMLTextAreaElement) return true;
  return target instanceof HTMLInputElement && TEXT_TYPES.includes(target.type);
}

/**
 * The current frame, drawn into canvases that are only ever shown.
 *
 * Saving means letting go of the file — `src` comes off the video and takes the
 * picture with it. Without this the stage would be black for as long as the
 * write takes. The sound simply stops and stays where it is; the picture should
 * do the same rather than disappear.
 *
 * Drawn into, never read back out: `toDataURL` throws on a canvas the browser
 * considers tainted, and one that is only ever displayed never has to answer
 * that question at all.
 */
export function freezeFrame(
  video: HTMLVideoElement | null,
  targets: Array<HTMLCanvasElement | null>,
): boolean {
  const width = video?.videoWidth ?? 0;
  const height = video?.videoHeight ?? 0;
  if (!video || !width || !height) return false;
  try {
    for (const canvas of targets) {
      if (!canvas) continue;
      canvas.width = width;
      canvas.height = height;
      canvas.getContext("2d")?.drawImage(video, 0, 0, width, height);
    }
    return true;
  } catch {
    // No frame decoded yet, or a source the canvas will not take. Then the
    // stage stays black exactly as it does today — not a reason to fail a save.
    return false;
  }
}
