/**
 * Takes away what makes a window feel like a web page: reload, find, print,
 * view source, history navigation, zoom, the WebView's own context menu, and a
 * file dropped in turning into a navigation.
 *
 * Installed once per window, in each of the three entry points.
 *
 * Only `preventDefault`, never `stopPropagation`: every other listener still
 * sees the key. That is what keeps the hotkey recorder in the settings working —
 * it has to be able to take F5 or F12 as an assignment, and it does, because
 * the WebView's reaction is what gets cancelled here, not the event.
 */

/** Keys that do something browser-ish on their own. */
const BARE = new Set([
  "F3", // find next
  "F5", // reload
  "F7", // caret browsing
  "BrowserBack",
  "BrowserForward",
  "BrowserRefresh",
  "BrowserSearch",
  "BrowserHome",
]);

/**
 * With Ctrl: find, print, save, open, history, downloads, new window, reload,
 * view source, zoom. Ctrl+A/C/V/X/Z/Y are not in here — text fields need them.
 */
const WITH_CTRL = new Set([
  "KeyF",
  "KeyG",
  "KeyH",
  "KeyJ",
  "KeyN",
  "KeyO",
  "KeyP",
  "KeyR",
  "KeyS",
  "KeyU",
  "F5",
  "Equal",
  "Minus",
  "Digit0",
  "NumpadAdd",
  "NumpadSubtract",
  "Numpad0",
]);

/** The developer tools. Left alone in a dev build, where they are the point. */
const DEVTOOLS_WITH_CTRL_SHIFT = new Set(["KeyI", "KeyJ", "KeyC"]);

function blocked(event: KeyboardEvent): boolean {
  const { code } = event;
  const dev = import.meta.env.DEV;
  if (code === "F12") return !dev;
  if (event.ctrlKey && event.shiftKey && DEVTOOLS_WITH_CTRL_SHIFT.has(code)) return !dev;
  if (BARE.has(code)) return true;
  if (event.ctrlKey && WITH_CTRL.has(code)) return true;
  // Back and forward.
  if (event.altKey && (code === "ArrowLeft" || code === "ArrowRight")) return true;
  return false;
}

/** The side buttons on the mouse: 3 is back, 4 is forward. */
const isSideButton = (event: MouseEvent) => event.button === 3 || event.button === 4;

export function installLockdown() {
  // Capture phase, so it holds even where a handler further in stops the event.
  window.addEventListener(
    "keydown",
    (event) => {
      if (blocked(event)) event.preventDefault();
    },
    true,
  );

  // Ctrl+wheel zooms. Tauri already switches the zoom hotkeys off; this is the
  // second lock on the same door.
  window.addEventListener(
    "wheel",
    (event) => {
      if (event.ctrlKey) event.preventDefault();
    },
    { passive: false, capture: true },
  );

  // WebView2 navigates on the release of a side button.
  for (const type of ["mousedown", "mouseup", "auxclick"] as const) {
    window.addEventListener(
      type,
      (event) => {
        if (isSideButton(event)) event.preventDefault();
      },
      true,
    );
  }

  // Bubble phase on `window`, the very last stop: the text-field menu in the
  // main window listens on `document` and steps aside once something has
  // prevented the default, so this must not get there before it does.
  window.addEventListener("contextmenu", (event) => event.preventDefault());

  // A file dropped anywhere would replace the page with it. The mixer's own
  // drag and drop has had its turn by the time the event arrives here.
  window.addEventListener("dragover", (event) => {
    if (event.defaultPrevented) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "none";
  });
  window.addEventListener("drop", (event) => event.preventDefault());
}
