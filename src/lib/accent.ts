import { listen } from "@tauri-apps/api/event";

import { api, inTauri } from "@/lib/ipc";

/**
 * The accent colour, in whichever window this runs — app, banner, console.
 *
 * The violet sits in five tokens (tokens.css): three in `@theme`, two beside
 * them for the steps that used to be written out as numbers — and the logo in
 * eleven more of its own. Left alone, they
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
  // The logo: a near-black disc with a hint of the colour, a rim that fades
  // from it into the dark, and the C and the play mark in it at full strength.
  ["--logo-disc-1", (c) => `color-mix(in oklab, ${c} 14%, #0c0b12)`],
  ["--logo-disc-2", (c) => `color-mix(in oklab, ${c} 8%, #09080e)`],
  ["--logo-disc-3", (c) => `color-mix(in oklab, ${c} 4%, #070609)`],
  ["--logo-rim-1", (c) => `color-mix(in oklab, ${c}, black 15%)`],
  ["--logo-rim-2", (c) => `color-mix(in oklab, ${c} 48%, #0a0a12)`],
  ["--logo-rim-3", (c) => `color-mix(in oklab, ${c} 28%, #0a0a12)`],
  ["--logo-mark-1", (c) => `color-mix(in oklab, ${c}, white 38%)`],
  ["--logo-mark-2", (c) => c],
  ["--logo-mark-3", (c) => `color-mix(in oklab, ${c}, black 22%)`],
  ["--logo-play-1", (c) => `color-mix(in oklab, ${c}, white 30%)`],
  ["--logo-play-2", (c) => `color-mix(in oklab, ${c}, black 12%)`],
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
