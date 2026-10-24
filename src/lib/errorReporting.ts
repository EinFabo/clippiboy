import { api, inTauri } from "./ipc";
import { describeError } from "./uiError";

/** Send an error to the app log; never throws, whatever state the page is in. */
export function reportUiError(error: unknown, componentStack?: string | null) {
  if (!inTauri) return;
  try {
    void api.uiError(describeError(error, componentStack)).catch(() => {});
  } catch {
    /* nothing left to do */
  }
}

/** The `createRoot` options and the rejection listener every window shares:
 *  an error React gave up on, or one it caught in a boundary, or a promise. */
export const rootErrorOptions = {
  onUncaughtError: (error: unknown, info: { componentStack?: string }) =>
    reportUiError(error, info.componentStack),
  onCaughtError: (error: unknown, info: { componentStack?: string }) =>
    reportUiError(error, info.componentStack),
};

export function installRejectionLogging() {
  window.addEventListener("unhandledrejection", (event) => {
    reportUiError(event.reason);
  });
}
