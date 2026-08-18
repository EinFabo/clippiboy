import { useMemo, useState } from "react";
import { useEngine } from "@/store";
import { Card, Pill } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { ClipPlayer } from "@/components/ClipPlayer";
import { IconFolder, IconScissors, IconTrash } from "@/components/icons";
import { api, fileUrl, inTauri } from "@/lib/ipc";
import { formatAgo, formatDuration, formatSize } from "@/lib/format";
import { cn } from "@/lib/cn";

export function Clips() {
  const { clips, deleteClip } = useEngine();
  const [query, setQuery] = useState("");
  const [game, setGame] = useState<string | null>(null);
  /** Welcher Clip im Player liegt — und ob gleich mit offenem Bearbeiten. */
  const [open, setOpen] = useState<{ index: number; editing: boolean } | null>(
    null,
  );

  const games = useMemo(
    () => [...new Set(clips.map((c) => c.game).filter(Boolean) as string[])],
    [clips],
  );

  const visible = clips.filter((c) => {
    const haystack = [
      c.path.split(/[\\/]/).pop() ?? "",
      c.title ?? "",
      c.description ?? "",
      c.game ?? "",
    ]
      .join(" ")
      .toLowerCase();
    const matchesQuery = !query || haystack.includes(query.toLowerCase());
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
                  onClick={() => setOpen({ index, editing: false })}
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
                <div
                  className="absolute top-3 right-3 flex gap-1.5 opacity-0 transition-opacity
                    group-hover:opacity-100"
                >
                  <IconAction
                    label="Im Ordner zeigen"
                    onClick={() => inTauri && api.revealClip(clip.id)}
                  >
                    <IconFolder className="h-4 w-4" />
                  </IconAction>
                  <IconAction
                    label="Clip löschen"
                    danger
                    onClick={() => deleteClip(clip.id)}
                  >
                    <IconTrash className="h-4 w-4" />
                  </IconAction>
                </div>
              </div>
              <div className="p-4">
                <p className="truncate text-sm font-medium">
                  {clip.title ?? clip.path.split(/[\\/]/).pop()}
                </p>
                <p className="mt-1 truncate text-xs text-ink-muted">
                  {clip.description ?? (
                    <>
                      {clip.game ? `${clip.game} · ` : ""}
                      {formatAgo(clip.createdAt)} · {clip.height}p ·{" "}
                      {formatSize(clip.sizeBytes)}
                    </>
                  )}
                </p>
                <div className="mt-3 flex gap-2">
                  <Button
                    size="sm"
                    onClick={() => setOpen({ index, editing: false })}
                  >
                    Ansehen
                  </Button>
                  <Button
                    size="sm"
                    variant="secondary"
                    icon={<IconScissors className="h-4 w-4" />}
                    onClick={() => setOpen({ index, editing: true })}
                  >
                    Bearbeiten
                  </Button>
                </div>
              </div>
            </Card>
          ))}
        </div>
      )}

      {open !== null && visible.length > 0 && (
        <ClipPlayer
          clips={visible}
          index={Math.min(open.index, visible.length - 1)}
          startEditing={open.editing}
          onIndexChange={(index) => setOpen({ ...open, index })}
          onClose={() => setOpen(null)}
          onDelete={deleteClip}
        />
      )}
    </div>
  );
}

/** Runder Knopf über dem Vorschaubild. */
function IconAction({
  label,
  danger,
  onClick,
  children,
}: {
  label: string;
  danger?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      aria-label={label}
      title={label}
      onClick={onClick}
      className={cn(
        "grid h-8 w-8 place-items-center rounded-pill bg-black/50 text-white/70",
        "backdrop-blur-md transition-colors",
        danger ? "hover:text-live" : "hover:text-white",
      )}
    >
      {children}
    </button>
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
