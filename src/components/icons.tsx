// Schlanke Inline-Icons (24er Grid, currentColor) — keine Icon-Library nötig.
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
