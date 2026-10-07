import { listen } from "@tauri-apps/api/event";
import { inTauri } from "./ipc";

// Der Kern meldet „main-hidden“, wenn das Hauptfenster in den Tray geht — über
// das ✕, Alt+F4 oder die Taskleiste, alle drei laufen durch `hide_to_tray`.
// Ein Clip, der dann weiterspielt, tönt aus dem Nichts.

let hidden = false;
const subscribers = new Set<() => void>();

if (inTauri) {
  void listen("main-hidden", () => {
    hidden = true;
    // Die Zusatzspuren folgen der Pause des Videos (`useClipMix`).
    for (const video of document.querySelectorAll("video")) video.pause();
    for (const cb of subscribers) cb();
  });
  // Zurück aus dem Tray kommt das Fenster mit Fokus.
  window.addEventListener("focus", () => {
    hidden = false;
  });
}

/** Whether the main window sits in the tray right now. */
export function isHidden(): boolean {
  return hidden;
}

/** Called each time the main window goes into the tray. */
export function onHidden(cb: () => void): () => void {
  subscribers.add(cb);
  return () => subscribers.delete(cb);
}
