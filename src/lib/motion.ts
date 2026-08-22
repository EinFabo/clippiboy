/**
 * The shared vocabulary for animations written in JS.
 *
 * CSS animations pull their curves from the tokens in styles/tokens.css, but
 * `element.animate()` resolves no variables — it needs the literal string. The
 * two have to stay in step, which is why they sit next to each other here.
 */

/** Matches --ease-out-soft. UI states. */
export const EASE_OUT_SOFT = "cubic-bezier(0.2, 0.8, 0.2, 1)";
/** Matches --ease-entrance. Things arriving. */
export const EASE_ENTRANCE = "cubic-bezier(0.16, 1, 0.3, 1)";
/** Matches --ease-exit. Things leaving. */
export const EASE_EXIT = "cubic-bezier(0.4, 0, 1, 1)";
/** Matches --ease-spring. One small overshoot, for a confirmed click. */
export const EASE_SPRING = "cubic-bezier(0.34, 1.56, 0.64, 1)";

/**
 * Whether the system asks for as little movement as possible.
 *
 * Read at the moment of the animation rather than watched: the setting changes
 * about once a year, and a listener on every animated component would cost
 * more than it saves. The CSS side is handled by the media query at the bottom
 * of styles/motion.css.
 */
export function prefersReducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

/**
 * `element.animate()` that quietly does nothing when movement is turned down —
 * or when the element has since left the document.
 *
 * No `fill` on purpose: every animation here starts and ends on the value the
 * stylesheet already holds, so letting it hand control back is the right
 * ending. A persisted fill would freeze the element and quietly beat every
 * later class change.
 */
export function animate(
  element: Element | null | undefined,
  frames: Keyframe[],
  options: KeyframeAnimationOptions,
): Animation | null {
  if (!element || !element.isConnected || prefersReducedMotion()) return null;
  return element.animate(frames, options);
}
