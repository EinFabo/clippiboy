import { cn } from "@/lib/cn";

/**
 * The dot that says something is running.
 *
 * A ring leaves it every two seconds and fades on the way out; the dot itself
 * stays put. That reads as "still going" — the pulsing opacity it replaces read
 * as "blinking at you", which is a different thing to say.
 */
export function LiveDot({ className }: { className?: string }) {
  return (
    <span className={cn("relative grid h-2 w-2 shrink-0 place-items-center", className)}>
      <span className="cb-live-ring absolute inset-0 rounded-pill bg-live" />
      <span className="absolute inset-0 rounded-pill bg-live" />
    </span>
  );
}
