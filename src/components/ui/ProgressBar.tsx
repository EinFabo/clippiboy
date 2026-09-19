import { cn } from "@/lib/cn";

/** A thin bar; `share` runs from 0 to 1. */
export function ProgressBar({ share, className }: { share: number; className?: string }) {
  return (
    <div className={cn("h-1 overflow-hidden rounded-pill bg-white/10", className)}>
      <div
        className="h-full rounded-pill bg-accent transition-[width] duration-200"
        style={{ width: `${Math.min(Math.max(share, 0), 1) * 100}%` }}
      />
    </div>
  );
}
