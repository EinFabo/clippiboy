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
