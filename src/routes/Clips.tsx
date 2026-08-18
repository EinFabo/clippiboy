import { useMemo, useState } from "react";
import { useEngine } from "@/store";
import { Card, Pill } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { ClipPlayer } from "@/components/ClipPlayer";
import { IconTrash } from "@/components/icons";
import { api, fileUrl, inTauri } from "@/lib/ipc";
import { formatAgo, formatDuration, formatSize } from "@/lib/format";
import { cn } from "@/lib/cn";

export function Clips() {
  const { clips, deleteClip } = useEngine();
  const [query, setQuery] = useState("");
  const [game, setGame] = useState<string | null>(null);
  const [playing, setPlaying] = useState<number | null>(null);

  const games = useMemo(
    () => [...new Set(clips.map((c) => c.game).filter(Boolean) as string[])],
    [clips],
  );

  const visible = clips.filter((c) => {
    const name = c.path.split("\\").pop() ?? "";
    const matchesQuery =
      !query ||
      name.toLowerCase().includes(query.toLowerCase()) ||
      (c.game ?? "").toLowerCase().includes(query.toLowerCase());
    return matchesQuery && (!game || c.game === game);
  });

  return (
    <div className="space-y-6">
      <header className="pt-10">
        <h1 className="display text-4xl">Clips</h1>
      </header>

      <div className="flex items-center gap-3">
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Suchen…"
          className="h-10 w-72 rounded-pill border border-line bg-surface px-5 text-sm
            outline-none transition-colors placeholder:text-ink-faint focus:border-line-strong"
        />
        <FilterPill active={!game} onClick={() => setGame(null)}>
          Alle
        </FilterPill>
        {games.map((g) => (
          <FilterPill key={g} active={game === g} onClick={() => setGame(g)}>
            {g}
          </FilterPill>
        ))}
        <span className="ml-auto text-xs text-ink-faint">
          {visible.length} Clips
        </span>
      </div>

      {visible.length === 0 ? (
        <Card className="grid h-56 place-items-center text-sm text-ink-muted">
          Keine Clips gefunden.
        </Card>
      ) : (
        <div className="grid grid-cols-3 gap-4 pb-10">
          {visible.map((clip, index) => (
            <Card key={clip.id} interactive className="group overflow-hidden">
              <div className="relative">
                <button
                  onClick={() => setPlaying(index)}
                  aria-label={`${clip.game ?? "Clip"} abspielen`}
                  className="relative block aspect-video w-full bg-gradient-to-br from-accent-deep/40 to-black"
                >
                  {clip.thumbPath && (
                    <img
                      src={fileUrl(clip.thumbPath)}
                      alt=""
                      className="h-full w-full object-cover"
                    />
                  )}
                  {/* Abspielsymbol nur beim Überfahren — sonst verdeckt es das Bild. */}
                  <span
                    className="absolute inset-0 grid place-items-center bg-black/30 opacity-0
                      transition-opacity duration-200 group-hover:opacity-100"
                  >
                    <span className="grid h-12 w-12 place-items-center rounded-pill bg-white/90 text-black">
                      <svg viewBox="0 0 24 24" className="h-5 w-5 translate-x-[1px]" fill="currentColor">
                        <path d="M7.5 5.2 19 12 7.5 18.8V5.2Z" />
                      </svg>
                    </span>
                  </span>
                  <span className="absolute bottom-3 left-3">
                    <Pill>{clip.game ?? "Unbekannt"}</Pill>
                  </span>
                  <span className="absolute right-3 bottom-3">
                    <Pill>{formatDuration(clip.durationMs)}</Pill>
                  </span>
                </button>
                <button
                  aria-label="Clip löschen"
                  onClick={() => deleteClip(clip.id)}
                  className="absolute top-3 right-3 grid h-8 w-8 place-items-center rounded-pill
                    bg-black/50 text-white/70 opacity-0 backdrop-blur-md transition-opacity
                    group-hover:opacity-100 hover:text-live"
                >
                  <IconTrash className="h-4 w-4" />
                </button>
              </div>
              <div className="p-4">
                <p className="truncate text-sm font-medium">
                  {clip.path.split("\\").pop()}
                </p>
                <p className="mt-1 text-xs text-ink-muted">
                  {clip.game ? `${clip.game} · ` : ""}
                  {formatAgo(clip.createdAt)} · {clip.height}p ·{" "}
                  {formatSize(clip.sizeBytes)}
                </p>
                <div className="mt-3 flex gap-2">
                  <Button size="sm" onClick={() => setPlaying(index)}>
                    Ansehen
                  </Button>
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={() => inTauri && api.revealClip(clip.id)}
                  >
                    Im Ordner zeigen
                  </Button>
                </div>
              </div>
            </Card>
          ))}
        </div>
      )}

      {playing !== null && visible.length > 0 && (
        <ClipPlayer
          clips={visible}
          index={Math.min(playing, visible.length - 1)}
          onIndexChange={setPlaying}
          onClose={() => setPlaying(null)}
          onDelete={deleteClip}
        />
      )}
    </div>
  );
}

function FilterPill({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className={cn(
        "h-8 rounded-pill px-4 text-[13px] font-medium transition-colors duration-150",
        active
          ? "bg-white text-black"
          : "border border-line bg-surface text-ink-muted hover:text-ink",
      )}
    >
      {children}
    </button>
  );
}
