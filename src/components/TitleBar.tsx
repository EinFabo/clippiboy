import { getCurrentWindow } from "@tauri-apps/api/window";
import { Mark } from "@/components/ui/Logo";

const win = () => getCurrentWindow();

function Ctrl({
  onClick,
  label,
  children,
  danger,
}: {
  onClick: () => void;
  label: string;
  children: React.ReactNode;
  danger?: boolean;
}) {
  return (
    <button
      aria-label={label}
      onClick={onClick}
      className={`grid h-8 w-8 place-items-center rounded-pill text-ink-muted transition-colors
        duration-150 hover:text-ink ${danger ? "hover:bg-live/80" : "hover:bg-elevated"}`}
    >
      {children}
    </button>
  );
}

/**
 * Above everything that covers the window — the player, the screenshot viewer,
 * the dialogs. Those used to lie over it, and while a clip was open the window
 * could be neither minimized nor maximized nor closed; a click up there landed
 * on the player's backdrop and closed the clip instead. The full views leave
 * the strip free for it (`pt-12` on their header).
 */
export function TitleBar() {
  return (
    <div
      data-tauri-drag-region
      className="fixed inset-x-0 top-0 z-[70] flex h-10 items-center justify-between px-3"
    >
      <div data-tauri-drag-region className="flex items-center gap-2 pl-1">
        {/* Inline rather than the file: it takes the accent colour. */}
        <Mark className="pointer-events-none h-[18px] w-[18px]" />
        <span className="text-[13px] font-semibold tracking-tight">ClippiBoy</span>
      </div>
      <div className="flex items-center gap-1">
        <Ctrl label="Minimize" onClick={() => win().minimize()}>
          <svg viewBox="0 0 12 12" className="h-3 w-3" stroke="currentColor" strokeWidth="1.2">
            <path d="M2.5 6h7" />
          </svg>
        </Ctrl>
        <Ctrl label="Maximize" onClick={() => win().toggleMaximize()}>
          <svg viewBox="0 0 12 12" className="h-3 w-3" fill="none" stroke="currentColor" strokeWidth="1.2">
            <rect x="2.5" y="2.5" width="7" height="7" rx="1.5" />
          </svg>
        </Ctrl>
        {/* close() rather than hide(): the Rust handler catches CloseRequested,
            hides the window, pauses a playing clip (`lib/hidden.ts`) and explains
            the tray the first time — that way ✕, Alt+F4 and the taskbar behave
            the same. Minimizing leaves a clip running. */}
        <Ctrl label="Close" danger onClick={() => void win().close()}>
          <svg viewBox="0 0 12 12" className="h-3 w-3" stroke="currentColor" strokeWidth="1.2">
            <path d="M3 3l6 6M9 3l-6 6" />
          </svg>
        </Ctrl>
      </div>
    </div>
  );
}
