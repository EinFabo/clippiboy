// Lean inline icons (24 grid, currentColor) — no icon library needed.
type P = { className?: string };
const base = "h-[18px] w-[18px]";

export const IconHome = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="1.8" stroke="currentColor">
    <path d="M3 10.5 12 3l9 7.5V20a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1v-9.5Z" strokeLinejoin="round" />
  </svg>
);

export const IconClips = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="1.8" stroke="currentColor">
    <rect x="3" y="5" width="18" height="14" rx="3" />
    <path d="M10 9.5v5l4.5-2.5L10 9.5Z" strokeLinejoin="round" />
  </svg>
);

export const IconAudio = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="1.8" stroke="currentColor" strokeLinecap="round">
    <path d="M5 10v4M9 6v12M13 8.5v7M17 4.5v15M21 10v4" />
  </svg>
);

export const IconRecord = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="1.8" stroke="currentColor">
    <circle cx="12" cy="12" r="8.5" />
    <circle cx="12" cy="12" r="3.5" fill="currentColor" stroke="none" />
  </svg>
);

export const IconSettings = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="1.8" stroke="currentColor">
    <circle cx="12" cy="12" r="3" />
    <path d="M19.4 15a1.6 1.6 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.6 1.6 0 0 0-2.7 1.1v.2a2 2 0 1 1-4 0v-.1A1.6 1.6 0 0 0 7 19.4a1.6 1.6 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1A1.6 1.6 0 0 0 2.3 14H2a2 2 0 1 1 0-4h.1A1.6 1.6 0 0 0 3.7 7a1.6 1.6 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1A1.6 1.6 0 0 0 9 2.3V2a2 2 0 1 1 4 0v.1A1.6 1.6 0 0 0 16.9 4l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.6 1.6 0 0 0-.3 1.8v.1a1.6 1.6 0 0 0 1.5 1h.2a2 2 0 1 1 0 4h-.1a1.6 1.6 0 0 0-1.5 1Z" strokeLinejoin="round" />
  </svg>
);

export const IconArrowUpRight = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="2" stroke="currentColor" strokeLinecap="round" strokeLinejoin="round">
    <path d="M7 17 17 7M8 7h9v9" />
  </svg>
);

export const IconScissors = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="1.8" stroke="currentColor" strokeLinecap="round">
    <circle cx="6" cy="18" r="2.5" />
    <circle cx="6" cy="6" r="2.5" />
    <path d="M8 7.5 20 18M20 6 8 16.5" />
  </svg>
);

export const IconMic = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="1.8" stroke="currentColor" strokeLinecap="round">
    <rect x="9" y="3" width="6" height="11" rx="3" />
    <path d="M5 11a7 7 0 0 0 14 0M12 18v3" />
  </svg>
);

export const IconSpeaker = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="1.8" stroke="currentColor" strokeLinejoin="round">
    <path d="M4 9.5h3.5L12 5.5v13L7.5 14.5H4v-5Z" />
    <path d="M15.5 9.5a4 4 0 0 1 0 5M18 7a7.5 7.5 0 0 1 0 10" strokeLinecap="round" />
  </svg>
);

export const IconApp = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="1.8" stroke="currentColor">
    <rect x="3" y="4" width="18" height="14" rx="2.5" />
    <path d="M8 21h8" strokeLinecap="round" />
  </svg>
);

export const IconPlus = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="2" stroke="currentColor" strokeLinecap="round">
    <path d="M12 5v14M5 12h14" />
  </svg>
);

export const IconTrash = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="1.8" stroke="currentColor" strokeLinecap="round">
    <path d="M4 7h16M9 7V5h6v2M6.5 7l.8 12a1 1 0 0 0 1 1h7.4a1 1 0 0 0 1-1l.8-12" />
  </svg>
);

/** The heart. Filled means marked — the difference has to be recognizable at a
    glance, without hunting for the colour. */
export const IconHeart = ({
  className = base,
  filled = false,
}: P & { filled?: boolean }) => (
  <svg
    viewBox="0 0 24 24"
    className={className}
    fill={filled ? "currentColor" : "none"}
    strokeWidth="1.8"
    stroke="currentColor"
    strokeLinejoin="round"
  >
    <path d="M12 20.3 4.6 13a4.6 4.6 0 0 1 6.5-6.5l.9.9.9-.9A4.6 4.6 0 1 1 19.4 13l-7.4 7.3Z" />
  </svg>
);

/** A screenshot: the camera, not the film. */
export const IconCamera = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" className={className} fill="none" strokeWidth="1.8" stroke="currentColor" strokeLinejoin="round">
    <path d="M3 8.5A1.5 1.5 0 0 1 4.5 7h2.2l1.3-2h7.9l1.3 2h2.3A1.5 1.5 0 0 1 21 8.5v9a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5v-9Z" />
    <circle cx="12" cy="13" r="3.4" />
  </svg>
);

export const IconPlay = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" className={className} fill="currentColor">
    <path d="M8 5.4 18.5 12 8 18.6V5.4Z" />
  </svg>
);

export const IconPencil = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="1.8" stroke="currentColor" strokeLinejoin="round">
    <path d="M4 20v-3.5L16 4.6a1.6 1.6 0 0 1 2.3 0l1.1 1.1a1.6 1.6 0 0 1 0 2.3L7.5 20H4Z" />
  </svg>
);

/** Two sheets on top of each other — copy. */
export const IconCopy = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="1.8" stroke="currentColor" strokeLinejoin="round">
    <rect x="9" y="9" width="11" height="11" rx="2.5" />
    <path d="M15 5.5A2.5 2.5 0 0 0 12.5 3h-7A2.5 2.5 0 0 0 3 5.5v7A2.5 2.5 0 0 0 5.5 15" strokeLinecap="round" />
  </svg>
);

/** Clipboard — paste. */
export const IconPaste = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="1.8" stroke="currentColor" strokeLinejoin="round">
    <path d="M9 4.5H7A2.5 2.5 0 0 0 4.5 7v12A2.5 2.5 0 0 0 7 21.5h10a2.5 2.5 0 0 0 2.5-2.5V7A2.5 2.5 0 0 0 17 4.5h-2" />
    <rect x="9" y="2.5" width="6" height="4" rx="1.5" />
  </svg>
);

/** Gestrichelter Rahmen — alles markieren. */
export const IconSelectAll = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="1.8" stroke="currentColor" strokeLinecap="round" strokeDasharray="3 3">
    <rect x="3.5" y="3.5" width="17" height="17" rx="3" />
  </svg>
);

export const IconFolder = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="1.8" stroke="currentColor" strokeLinejoin="round">
    <path d="M3 7.5A1.5 1.5 0 0 1 4.5 6h4l2 2.5h7A1.5 1.5 0 0 1 19 10v7a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 3 17V7.5Z" />
  </svg>
);

export const IconSearch = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="1.8" stroke="currentColor" strokeLinecap="round">
    <circle cx="11" cy="11" r="6.5" />
    <path d="m16 16 4.5 4.5" />
  </svg>
);

export const IconClose = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="2" stroke="currentColor" strokeLinecap="round">
    <path d="m6 6 12 12M18 6 6 18" />
  </svg>
);

export const IconCheck = ({ className = base }: P) => (
  <svg viewBox="0 0 24 24" fill="none" className={className} strokeWidth="2" stroke="currentColor" strokeLinecap="round" strokeLinejoin="round">
    <path d="m5 12.5 4.5 4.5L19 7" />
  </svg>
);
