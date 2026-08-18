import type { HTMLAttributes, ReactNode } from "react";
import { cn } from "@/lib/cn";

interface CardProps extends HTMLAttributes<HTMLDivElement> {
  interactive?: boolean;
}

export function Card({ interactive, className, children, ...rest }: CardProps) {
  return (
    <div
      className={cn(
        "rounded-card border border-line bg-surface",
        interactive &&
          "transition-[background-color,border-color,transform] duration-200 ease-[var(--ease-out-soft)] hover:border-line-strong hover:bg-elevated",
        className,
      )}
      {...rest}
    >
      {children}
    </div>
  );
}

export function Pill({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 rounded-pill px-3 py-1 text-xs font-medium",
        "bg-black/45 text-ink backdrop-blur-md",
        className,
      )}
    >
      {children}
    </span>
  );
}

export function SectionTitle({
  title,
  action,
}: {
  title: string;
  action?: ReactNode;
}) {
  return (
    <div className="mb-4 flex items-end justify-between">
      <h2 className="display text-2xl">{title}</h2>
      {action}
    </div>
  );
}
