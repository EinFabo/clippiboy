import { cn } from "@/lib/cn";
import {
  IconAudio,
  IconClips,
  IconHome,
  IconRecord,
  IconSettings,
} from "./icons";
import { useEngine } from "@/store";

export type Route = "dashboard" | "clips" | "audio" | "recording" | "settings";

const items: Array<{ id: Route; label: string; icon: typeof IconHome }> = [
  { id: "dashboard", label: "Overview", icon: IconHome },
  { id: "clips", label: "Clips", icon: IconClips },
  { id: "audio", label: "Audio", icon: IconAudio },
  { id: "recording", label: "Recording", icon: IconRecord },
  { id: "settings", label: "Settings", icon: IconSettings },
];

export function NavBar({
  route,
  onNavigate,
}: {
  route: Route;
  onNavigate: (r: Route) => void;
}) {
  const bufferActive = useEngine((s) => s.bufferActive);

  return (
    <nav className="absolute inset-x-0 top-12 z-40 flex justify-center px-8">
      <div
        className="flex items-center gap-1 rounded-pill border border-white/10 p-1.5 pr-2 shadow-[0_8px_32px_rgba(0,0,0,0.4)]"
        style={{ background: "var(--glass)", backdropFilter: "blur(20px)" }}
      >
        {items.map(({ id, label, icon: Icon }) => (
          <button
            key={id}
            onClick={() => onNavigate(id)}
            className={cn(
              "inline-flex h-9 items-center gap-2 rounded-pill px-4 text-[13px] font-medium",
              "transition-colors duration-150 ease-[var(--ease-out-soft)]",
              route === id
                ? "bg-white/12 text-ink"
                : "text-ink-muted hover:text-ink",
            )}
          >
            <Icon className="h-4 w-4" />
            {label}
          </button>
        ))}

        <span className="mx-1 h-5 w-px bg-white/10" />

        <span
          className={cn(
            "inline-flex h-9 items-center gap-2 rounded-pill px-4 text-[13px] font-medium",
            bufferActive ? "text-ink" : "text-ink-faint",
          )}
          title={bufferActive ? "Replay buffer running" : "Replay buffer off"}
        >
          <span
            className={cn(
              "h-2 w-2 rounded-pill",
              bufferActive ? "animate-pulse bg-live" : "bg-line-strong",
            )}
          />
          {bufferActive ? "Buffer on" : "Buffer off"}
        </span>
      </div>
    </nav>
  );
}
