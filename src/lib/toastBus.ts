/** A toast raised by the window itself, as opposed to a `notice` from the core. */
export interface ToastNotice {
  kind: "ok" | "error";
  message: string;
}

const listeners = new Set<(n: ToastNotice) => void>();

/** Shows a toast in this window. Without a listener (no `Toasts` mounted) it is dropped. */
export function toast(kind: "ok" | "error", message: string): void {
  for (const cb of [...listeners]) cb({ kind, message });
}

/** Subscribes to toasts; returns the function that unsubscribes. */
export function onToast(cb: (n: ToastNotice) => void): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}
