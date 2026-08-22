import { useEffect, useRef } from "react";
import { cn } from "@/lib/cn";
import { IconCheck, IconClose } from "../icons";

/**
 * The question before a clip is deleted, standing in the place of the control
 * that asked it.
 *
 * Same idea as the filter bar's confirmation: no dialog over the page, just the
 * button turning into a question. What is at stake here is larger, though — the
 * file is gone from the disk afterwards — so unlike that one it does **not**
 * focus its own confirm button. A stray Enter or Space must not be able to
 * finish what a stray click started.
 */
export function ConfirmDelete({
  question = "Delete?",
  origin = "left",
  onConfirm,
  onCancel,
  className,
}: {
  question?: string;
  /** Which end it grows out of — the side the control it replaced sat on. */
  origin?: "left" | "right";
  onConfirm: () => void;
  onCancel: () => void;
  className?: string;
}) {
  // Through a ref, so a caller writing its handler inline does not resubscribe
  // the listener on every render.
  const cancel = useRef(onCancel);
  cancel.current = onCancel;

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") cancel.current();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div
      style={{ transformOrigin: `${origin} center` }}
      onClick={(event) => event.stopPropagation()}
      className={cn(
        "cb-chip-expand flex h-8 shrink-0 items-center gap-1 rounded-pill border",
        "border-live/40 bg-live/20 pl-3 text-[13px] font-medium text-live backdrop-blur-md",
        className,
      )}
    >
      <span className="whitespace-nowrap">{question}</span>
      <button
        aria-label="Confirm deletion"
        title="Delete for good"
        onClick={onConfirm}
        className="ml-1 grid h-6 w-6 shrink-0 place-items-center rounded-pill hover:bg-live/30"
      >
        <IconCheck className="h-3.5 w-3.5" />
      </button>
      <button
        aria-label="Keep the clip"
        title="Keep it — Escape does the same"
        onClick={onCancel}
        className="mr-1 grid h-6 w-6 shrink-0 place-items-center rounded-pill text-white/70
          hover:bg-white/15 hover:text-white"
      >
        <IconClose className="h-3 w-3" />
      </button>
    </div>
  );
}
