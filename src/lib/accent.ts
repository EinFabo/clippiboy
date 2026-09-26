import { listen } from "@tauri-apps/api/event";

import { api, inTauri } from "@/lib/ipc";
import type { Accent, AppConfig } from "@/lib/types";

/**
 * The accent colour, in whichever window this runs — app, banner, console.
 *
 * The violet sits in five tokens (tokens.css): three in `@theme`, two beside
 * them for the steps that used to be written out as numbers — and the logo in
 * eleven more of its own. Left alone, they stay exactly the violet. An accent of
 * one's own sets them all on `<html>`, mixed from the colour — one colour
 * chosen, and the lighter and darker steps still sit to it the way they sat to
 * the violet.
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

/**
 * For a gradient: the steps every gradient ends on, taken from the second
 * colour instead. The app's gradients all run from the bright or strong steps
 * to the deep and dark ones — the header, the console's glow, the logo's C — so
 * this alone makes each of them run from one colour to the other. Lighter than
 * in {@link STEPS}: there they only had to be a darker violet, here the second
 * colour has to be seen.
 */
const SECOND: Array<[string, (color: string) => string]> = [
  ["--color-accent-deep", (c) => `color-mix(in oklab, ${c}, black 30%)`],
  ["--color-accent-night", (c) => `color-mix(in oklab, ${c} 45%, #0a0a12)`],
  ["--logo-rim-2", (c) => `color-mix(in oklab, ${c} 60%, #0a0a12)`],
  ["--logo-rim-3", (c) => `color-mix(in oklab, ${c} 35%, #0a0a12)`],
  ["--logo-mark-3", (c) => c],
  ["--logo-play-2", (c) => c],
];

const VIOLET = "#8b5cf6";

/** Only this form is set — the core keeps nothing else either. */
const HEX = /^#[0-9a-f]{6}$/i;

/** One turn of the hue, in milliseconds. */
const PERIOD = { slow: 30_000, medium: 15_000 };

/** How often the turning hue is written while the main window shows. */
const FRAME_MS = 80;

/**
 * The hue right now. Taken from the clock rather than counted, so every window
 * arrives at the same one — the console opened mid-turn shows the colour the
 * app shows at that moment.
 */
function hueNow(accent: Accent): string {
  const turn = (Date.now() % PERIOD[accent.speed]) / PERIOD[accent.speed];
  // Saturation and lightness of the violet, so the other hues sit as bright.
  return `hsl(${Math.round(turn * 360)} 90% 66%)`;
}

function paint(steps: Array<[string, (color: string) => string]>, color: string) {
  const root = document.documentElement.style;
  for (const [name, step] of steps) root.setProperty(name, step(color));
}

function clear() {
  const root = document.documentElement.style;
  for (const [name] of STEPS) root.removeProperty(name);
}

/** Put this accent on the window, as it stands at this moment. */
export function applyAccent(accent: Accent) {
  const first = accent.color && HEX.test(accent.color) ? accent.color : null;
  if (accent.mode === "rgb") {
    paint(STEPS, hueNow(accent));
  } else if (accent.mode === "gradient") {
    const second = accent.color2 && HEX.test(accent.color2) ? accent.color2 : VIOLET;
    paint(STEPS, first ?? VIOLET);
    paint(SECOND, second);
  } else if (first) {
    paint(STEPS, first);
  } else {
    clear();
  }
}

export function accentOf(config: AppConfig): Accent {
  return {
    mode: config.accentMode,
    color: config.accentColor,
    color2: config.accentColor2,
    speed: config.rgbSpeed,
  };
}

/**
 * Take the accent from the config now, and follow it when it changes. Installed
 * once per window, in each of the three entry points, like the lockdown.
 *
 * `turning` is for the main window alone. In RGB it moves the hue on, a few
 * times a second, as long as the window is showing. The banner and the console
 * stand over a game and draw without the GPU there (overlay.rs) — a colour that
 * never stops would cost CPU in the middle of play. They take the hue of the
 * moment whenever they come up, and then hold it.
 */
export function installAccent({ turning = false }: { turning?: boolean } = {}) {
  if (!inTauri) return;
  let current: Accent | null = null;
  const take = (accent: Accent) => {
    current = accent;
    applyAccent(accent);
  };
  const refresh = () => {
    if (current?.mode === "rgb" && !document.hidden) applyAccent(current);
  };

  void api
    .getConfig()
    .then((config) => take(accentOf(config)))
    .catch(() => {});
  void listen<Accent>("accent-changed", (event) => take(event.payload));

  if (turning) {
    window.setInterval(refresh, FRAME_MS);
  } else {
    // The moments these windows come up.
    document.addEventListener("visibilitychange", refresh);
    void listen("console-opened", refresh);
    void listen("overlay-banner", refresh);
  }
}
