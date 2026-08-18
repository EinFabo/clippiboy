import { getCurrentWindow } from "@tauri-apps/api/window";
import logo from "@/assets/logo.svg";

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

export function TitleBar() {
  return (
    <div
      data-tauri-drag-region
      className="fixed inset-x-0 top-0 z-50 flex h-10 items-center justify-between px-3"
    >
      <div data-tauri-drag-region className="flex items-center gap-2 pl-1">
        <img src={logo} alt="" className="h-[18px] w-[18px]" draggable={false} />
        <span className="text-[13px] font-semibold tracking-tight">ClippiBoy</span>
      </div>
      <div className="flex items-center gap-1">
        <Ctrl label="Minimieren" onClick={() => win().minimize()}>
          <svg viewBox="0 0 12 12" className="h-3 w-3" stroke="currentColor" strokeWidth="1.2">
            <path d="M2.5 6h7" />
          </svg>
        </Ctrl>
        <Ctrl label="Maximieren" onClick={() => win().toggleMaximize()}>
          <svg viewBox="0 0 12 12" className="h-3 w-3" fill="none" stroke="currentColor" strokeWidth="1.2">
            <rect x="2.5" y="2.5" width="7" height="7" rx="1.5" />
          </svg>
        </Ctrl>
        {/* close() statt hide(): der Rust-Handler fängt CloseRequested ab, versteckt
            das Fenster und erklärt beim ersten Mal das Tray — so verhalten sich ✕
            und Alt+F4 gleich. */}
        <Ctrl label="Schließen" danger onClick={() => win().close()}>
          <svg viewBox="0 0 12 12" className="h-3 w-3" stroke="currentColor" strokeWidth="1.2">
            <path d="M3 3l6 6M9 3l-6 6" />
          </svg>
        </Ctrl>
      </div>
    </div>
  );
}
