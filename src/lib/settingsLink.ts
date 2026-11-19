import { useEngine, type SettingsTab } from "@/store";

/** What App listens for: the route is its own state, there is no router. */
export const OPEN_SETTINGS_EVENT = "clippiboy:open-settings";

/** Goes to Settings on the given tab, from wherever the link stands. */
export function openSettings(tab: SettingsTab): void {
  useEngine.getState().setSettingsTab(tab);
  window.dispatchEvent(new Event(OPEN_SETTINGS_EVENT));
}
