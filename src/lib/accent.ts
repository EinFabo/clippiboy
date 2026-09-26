import { listen } from "@tauri-apps/api/event";

import { api, inTauri } from "@/lib/ipc";

/**
 * The accent colour, in whichever window this runs — app, banner, console.
 *
 * The violet sits in five tokens (tokens.css): three in `@theme`, two beside
 * them for the steps that used to be written out as numbers. Left alone, they
 * stay exactly the violet. An accent of one's own sets all five on `<html>`,
 * the other four mixed from it — one colour chosen, and the lighter and darker
 * steps still sit to it the way they sat to the violet.
 */
const STEPS: Array<[string, (color: string) => string]> = [
  ["--color-accent", (c) => c],
  ["--color-accent-bright", (c) => `color-mix(in oklab, ${c}, white 30%)`],
  ["--color-accent-strong", (c) => `color-mix(in oklab, ${c}, black 12%)`],
  ["--color-accent-deep", (c) => `color-mix(in oklab, ${c}, black 55%)`],
  ["--color-accent-night", (c) => `color-mix(in oklab, ${c} 34%, #0a0a12)`],
];

/** Only this form is set — the core keeps nothing else either. */
const HEX = /^#[0-9a-f]{6}$/i;

export function applyAccent(color: string | null | undefined) {
  const root = document.documentElement.style;
  for (const [name, step] of STEPS) {
    if (color && HEX.test(color)) root.setProperty(name, step(color));
    else root.removeProperty(name);
  }
}

/**
 * Take the accent from the config now, and follow it when it changes. Installed
 * once per window, in each of the three entry points, like the lockdown.
 */
export function installAccent() {
  if (!inTauri) return;
  void api
    .getConfig()
    .then((config) => applyAccent(config.accentColor))
    .catch(() => {});
  void listen<string | null>("accent-changed", (event) => applyAccent(event.payload));
}
