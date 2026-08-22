import { useEffect, useRef } from "react";
import { cn } from "@/lib/cn";
import { IconHeart } from "../icons";
import { animate, EASE_OUT_SOFT, EASE_SPRING } from "@/lib/motion";

/**
 * The heart, and the small moment of setting it.
 *
 * One ring leaves it and is gone — not a shower of particles. The app is
 * confirming a decision, not throwing a party. Taking the heart off gets the
 * mirror of that: a short dip, no ring, because nothing was gained.
 *
 * Nothing fires on mount, only on a change: opening the gallery must not set
 * off every favourite in it at once.
 */
export function HeartBurst({
  favorite,
  className,
}: {
  favorite: boolean;
  className?: string;
}) {
  const ring = useRef<HTMLSpanElement>(null);
  const icon = useRef<HTMLSpanElement>(null);
  const was = useRef(favorite);

  useEffect(() => {
    if (was.current === favorite) return;
    was.current = favorite;

    if (favorite) {
      animate(
        icon.current,
        [
          { transform: "scale(1)" },
          { transform: "scale(1.3)", offset: 0.4 },
          { transform: "scale(1)" },
        ],
        { duration: 380, easing: EASE_SPRING },
      );
      animate(
        ring.current,
        [
          { opacity: 0.5, transform: "scale(0.6)" },
          { opacity: 0, transform: "scale(1.8)" },
        ],
        { duration: 500, easing: "ease-out" },
      );
    } else {
      animate(
        icon.current,
        [
          { transform: "scale(1)" },
          { transform: "scale(0.85)", offset: 0.4 },
          { transform: "scale(1)" },
        ],
        { duration: 260, easing: EASE_OUT_SOFT },
      );
    }
  }, [favorite]);

  return (
    <span className="relative grid place-items-center">
      {/* `border-current` so the ring is whatever colour the heart is at that
          moment — red on the tile, black on the filled button in the player. */}
      <span
        ref={ring}
        aria-hidden
        style={{ opacity: 0 }}
        className="pointer-events-none absolute -inset-1 rounded-pill border border-current"
      />
      <span ref={icon} className="grid place-items-center">
        <IconHeart filled={favorite} className={cn("h-4 w-4", className)} />
      </span>
    </span>
  );
}
