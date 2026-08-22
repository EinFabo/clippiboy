import { useEffect, useRef, useState } from "react";
import { prefersReducedMotion } from "./motion";

/**
 * Runs a number up to its new value instead of letting it jump.
 *
 * Only for real jumps. The replay buffer counts up once a second, and
 * animating every tick would turn a quiet number into a flickering one — so
 * anything within `threshold` is taken straight. What is left is what actually
 * deserves the movement: switching the buffer on, a clip arriving, a filter
 * changing the count.
 */
export function useCountUp(
  target: number,
  { duration = 400, threshold = 2 }: { duration?: number; threshold?: number } = {},
): number {
  const [shown, setShown] = useState(target);
  const frame = useRef(0);
  const current = useRef(target);

  useEffect(() => {
    const from = current.current;
    const jump = Math.abs(target - from);

    if (!Number.isFinite(target) || jump <= threshold || prefersReducedMotion()) {
      current.current = target;
      setShown(target);
      return;
    }

    const started = performance.now();
    const step = (now: number) => {
      const t = Math.min(1, (now - started) / duration);
      // Ease-out, the same shape the rest of the app arrives with.
      const eased = 1 - Math.pow(1 - t, 3);
      const value = from + (target - from) * eased;
      current.current = value;
      setShown(value);
      if (t < 1) frame.current = requestAnimationFrame(step);
      else current.current = target;
    };
    frame.current = requestAnimationFrame(step);

    return () => cancelAnimationFrame(frame.current);
  }, [target, duration, threshold]);

  return shown;
}
