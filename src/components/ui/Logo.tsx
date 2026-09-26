import { useId } from "react";

/** The C, as it is drawn in assets/logo.svg. */
const C = "M177.03 86.86 A64 64 0 1 0 177.03 169.14";
/** The same arc backwards, so a dash along it starts at the lower end. */
export const ARC = "M177.03 169.14 A64 64 0 1 1 177.03 86.86";
const PLAY = "M111.6 102.1 L143.2 128 L111.6 153.9 Z";

/**
 * ClippiBoy's logo, inline so parts of it can be moved.
 *
 * Its colours come from the `--logo-*` tokens (tokens.css), so it follows the
 * accent like everything else — see lib/accent.ts.
 *
 * The gradient ids are made unique per instance: two of these on screen at once
 * with the same ids would have the second one quietly borrow the first one's
 * definitions.
 */
export function Mark({ className, arc }: { className?: string; arc?: React.ReactNode }) {
  const unique = useId().replace(/:/g, "");
  const disc = `${unique}-disc`;
  const rim = `${unique}-rim`;
  const mark = `${unique}-mark`;
  const play = `${unique}-play`;

  return (
    <svg viewBox="0 0 256 256" className={className} aria-hidden>
      <defs>
        <radialGradient id={disc} cx="34%" cy="24%" r="92%">
          <stop offset="0" style={{ stopColor: "var(--logo-disc-1)" }} />
          <stop offset=".55" style={{ stopColor: "var(--logo-disc-2)" }} />
          <stop offset="1" style={{ stopColor: "var(--logo-disc-3)" }} />
        </radialGradient>
        <linearGradient id={rim} x1="0" y1="0" x2=".45" y2="1">
          <stop offset="0" style={{ stopColor: "var(--logo-rim-1)" }} />
          <stop offset=".5" style={{ stopColor: "var(--logo-rim-2)" }} />
          <stop offset="1" style={{ stopColor: "var(--logo-rim-3)" }} />
        </linearGradient>
        <linearGradient id={mark} x1=".08" y1=".05" x2=".92" y2=".95">
          <stop offset="0" style={{ stopColor: "var(--logo-mark-1)" }} />
          <stop offset=".45" style={{ stopColor: "var(--logo-mark-2)" }} />
          <stop offset="1" style={{ stopColor: "var(--logo-mark-3)" }} />
        </linearGradient>
        <linearGradient id={play} x1=".1" y1="0" x2=".9" y2="1">
          <stop offset="0" style={{ stopColor: "var(--logo-play-1)" }} />
          <stop offset="1" style={{ stopColor: "var(--logo-play-2)" }} />
        </linearGradient>
      </defs>

      <circle cx="128" cy="128" r="125" fill={`url(#${disc})`} />
      <circle cx="128" cy="128" r="123.5" fill="none" stroke={`url(#${rim})`} strokeWidth="3" />
      {/* With an arc on top the C steps back and becomes the track it runs on. */}
      <path
        d={C}
        fill="none"
        stroke={`url(#${mark})`}
        strokeWidth="28"
        strokeLinecap="round"
        opacity={arc ? 0.26 : 1}
      />
      {arc && <g stroke={`url(#${mark})`}>{arc}</g>}
      <path
        d={PLAY}
        fill={`url(#${play})`}
        stroke={`url(#${play})`}
        strokeWidth="9"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/**
 * The logo as pixels, in the colours on screen right now — for the tray and the
 * task bar, which Windows draws and which know nothing about CSS.
 *
 * Drawn from the same paths and stops as {@link Mark}, so the icons match the
 * logo inside the app in every style; an earlier recolouring of the tray image
 * in the core laid its own gradient over the whole icon and never looked the
 * same. `rainbow` is for the RGB style: the icons do not turn, so they carry the
 * whole wheel at once instead.
 */
export async function renderLogo(size: number, rainbow: boolean): Promise<Uint8ClampedArray> {
  const color = (name: string) => resolve(`var(${name})`);
  const wheel = (lightness: number) =>
    [0, 60, 120, 180, 240, 300, 360]
      .map((hue, at) => `<stop offset="${at / 6}" stop-color="hsl(${hue} 90% ${lightness}%)"/>`)
      .join("");
  const stops = (names: string[], offsets: number[]) =>
    names.map((name, at) => `<stop offset="${offsets[at]}" stop-color="${color(name)}"/>`).join("");

  const disc = rainbow
    ? `<stop offset="0" stop-color="#17161d"/><stop offset=".55" stop-color="#0f0e14"/><stop offset="1" stop-color="#08080a"/>`
    : stops(["--logo-disc-1", "--logo-disc-2", "--logo-disc-3"], [0, 0.55, 1]);
  const rim = rainbow ? wheel(45) : stops(["--logo-rim-1", "--logo-rim-2", "--logo-rim-3"], [0, 0.5, 1]);
  const mark = rainbow
    ? wheel(66)
    : stops(["--logo-mark-1", "--logo-mark-2", "--logo-mark-3"], [0, 0.45, 1]);
  const play = rainbow ? wheel(72) : stops(["--logo-play-1", "--logo-play-2"], [0, 1]);

  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="${size}" height="${size}">
<defs>
<radialGradient id="d" cx="34%" cy="24%" r="92%">${disc}</radialGradient>
<linearGradient id="r" x1="0" y1="0" x2=".45" y2="1">${rim}</linearGradient>
<linearGradient id="m" x1=".08" y1=".05" x2=".92" y2=".95">${mark}</linearGradient>
<linearGradient id="p" x1=".1" y1="0" x2=".9" y2="1">${play}</linearGradient>
</defs>
<circle cx="128" cy="128" r="125" fill="url(#d)"/>
<circle cx="128" cy="128" r="123.5" fill="none" stroke="url(#r)" stroke-width="3"/>
<path d="${C}" fill="none" stroke="url(#m)" stroke-width="28" stroke-linecap="round"/>
<path d="${PLAY}" fill="url(#p)" stroke="url(#p)" stroke-width="9" stroke-linejoin="round"/>
</svg>`;

  const picture = new Image(size, size);
  picture.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
  await picture.decode();
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("no canvas");
  ctx.drawImage(picture, 0, 0, size, size);
  return ctx.getImageData(0, 0, size, size).data;
}

/**
 * A colour as the browser computes it. A picture drawn from a data URL is a
 * document of its own and cannot see this page's custom properties, so every
 * `var()` has to be settled before it goes in.
 */
function resolve(value: string): string {
  const probe = document.createElement("span");
  probe.style.color = value;
  probe.style.display = "none";
  document.body.append(probe);
  const color = getComputedStyle(probe).color;
  probe.remove();
  return color;
}
