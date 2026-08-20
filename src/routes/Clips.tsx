import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useEngine } from "@/store";
import { Card, Pill } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { ClipPlayer } from "@/components/ClipPlayer";
import {
  IconCheck,
  IconClose,
  IconFolder,
  IconScissors,
  IconSearch,
  IconTrash,
} from "@/components/icons";
import type { Route } from "@/components/NavBar";
import { api, fileUrl, inTauri } from "@/lib/ipc";
import { clipName, fileName, formatAgo, formatDuration, formatSize } from "@/lib/format";
import { cn } from "@/lib/cn";

/**
 * Der Filter für Clips ohne Spiel.
 *
 * `null` heißt „alle", ein Spielname filtert auf dieses Spiel. Der leere String
 * kann für keines davon stehen: Der Kern macht aus einem leeren Spielnamen
 * beim Speichern `null`. Deshalb taugt er als dritter Zustand.
 */
const NO_GAME = "";

export function Clips({ onNavigate }: { onNavigate: (r: Route) => void }) {
  const { clips, deleteClip, clearGame } = useEngine();
  const [query, setQuery] = useState("");
  const [game, setGame] = useState<string | null>(null);
  /** Welcher Clip im Player liegt. Bearbeitet wird dort immer. */
  const [open, setOpen] = useState<number | null>(null);

  /** Spiele mit Anzahl, häufigste zuerst — die Leiste soll oben stehen haben,
      wonach auch wirklich gefiltert wird. */
  const games = useMemo(() => {
    const counts = new Map<string, number>();
    for (const clip of clips) {
      if (clip.game) counts.set(clip.game, (counts.get(clip.game) ?? 0) + 1);
    }
    return [...counts]
      .map(([name, count]) => ({ name, count }))
      .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name, "de"));
  }, [clips]);

  const untagged = useMemo(() => clips.filter((c) => !c.game).length, [clips]);

  // Wird das Spiel eines Clips umbenannt oder entfernt, verschwindet sein
  // Filter — ohne das bliebe die Galerie leer und niemand wüsste, warum.
  useEffect(() => {
    if (game === null) return;
    const gone =
      game === NO_GAME ? untagged === 0 : !games.some((g) => g.name === game);
    if (gone) setGame(null);
  }, [game, games, untagged]);

  const visible = clips.filter((c) => {
    const haystack = [
      fileName(c.path),
      c.title ?? "",
      c.description ?? "",
      c.game ?? "",
    ]
      .join(" ")
      .toLowerCase();
    const matchesQuery = !query || haystack.includes(query.toLowerCase());
    const matchesGame =
      game === null ? true : game === NO_GAME ? !c.game : c.game === game;
    return matchesQuery && matchesGame;
  });

  const filtered = query !== "" || game !== null;

  return (
    <div className="space-y-6">
      <header className="pt-10">
        <h1 className="display text-4xl">Clips</h1>
      </header>

      <div className="space-y-3">
        <div className="flex items-center gap-3">
          <SearchField value={query} onChange={setQuery} />
          <span className="ml-auto shrink-0 text-xs text-ink-faint">
            {filtered
              ? `${visible.length} von ${clips.length} Clips`
              : `${clips.length} Clips`}
          </span>
        </div>

        <GameFilters
          games={games}
          untagged={untagged}
          total={clips.length}
          active={game}
          onSelect={setGame}
          onRemove={clearGame}
        />
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
                  onClick={() => setOpen(index)}
                  aria-label={`${clip.game ?? "Clip"} abspielen`}
                  className="relative block aspect-video w-full bg-gradient-to-br from-accent-deep/40 to-black"
                >
                  {clip.thumbPath && (
                    <img
                      // Nach einem Schnitt steht unter demselben Pfad ein neues
                      // Bild. Ohne den Anhang zeigte der WebView weiter das aus
                      // seinem Zwischenspeicher — also eine Stelle, die im Clip
                      // gar nicht mehr vorkommt.
                      src={`${fileUrl(clip.thumbPath)}?v=${clip.sizeBytes}`}
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
                  {/* Beide Marken in einer Zeile: Ein langer Spielname schiebt
                      sich sonst unter die Dauer statt sich zu kürzen. */}
                  <span className="absolute inset-x-3 bottom-3 flex items-end justify-between gap-2">
                    <Pill className="min-w-0">
                      <span className="min-w-0 truncate">{clip.game ?? "Unbekannt"}</span>
                    </Pill>
                    <span className="flex shrink-0 items-center gap-1.5">
                      {/* Nur bei echtem Zuschnitt: Eine geänderte Mischung
                          sieht man dem Clip nicht an, aber seine Länge schon —
                          und dass das Original noch daneben liegt, ist die
                          Auskunft, die hier zählt. */}
                      {clip.original && (
                        <Pill title="Zugeschnitten — das Original liegt daneben">
                          <IconScissors className="h-3 w-3" />
                        </Pill>
                      )}
                      <Pill>{formatDuration(clip.durationMs)}</Pill>
                    </span>
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
                  {clipName(clip)}
                </p>
                <p className="mt-1 truncate text-xs text-ink-muted">
                  {clip.game ? `${clip.game} · ` : ""}
                  {formatAgo(clip.createdAt)} · {clip.height}p ·{" "}
                  {formatSize(clip.sizeBytes)}
                </p>
                {clip.description && (
                  <p className="mt-1 truncate text-xs text-ink-faint">
                    {clip.description}
                  </p>
                )}
                {/* Nur ein Knopf: Ansehen und Bearbeiten sind derselbe
                    Bildschirm geworden. */}
                <div className="mt-3">
                  <Button size="sm" onClick={() => setOpen(index)}>
                    Öffnen
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
          index={Math.min(open, visible.length - 1)}
          onIndexChange={setOpen}
          onClose={() => setOpen(null)}
          onDelete={deleteClip}
          onOpenMixer={() => onNavigate("audio")}
        />
      )}
    </div>
  );
}

function SearchField({
  value,
  onChange,
}: {
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <div className="relative w-80">
      <IconSearch className="pointer-events-none absolute top-1/2 left-4 h-4 w-4 -translate-y-1/2 text-ink-faint" />
      <input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => e.key === "Escape" && onChange("")}
        placeholder="Suchen…"
        className="h-10 w-full rounded-pill border border-line bg-surface pr-10 pl-10 text-sm
          outline-none transition-colors placeholder:text-ink-faint focus:border-line-strong"
      />
      {value && (
        <button
          aria-label="Suche leeren"
          onClick={() => onChange("")}
          className="absolute top-1/2 right-3 grid h-6 w-6 -translate-y-1/2 place-items-center
            rounded-pill text-ink-faint transition-colors hover:bg-hover hover:text-ink"
        >
          <IconClose className="h-3.5 w-3.5" />
        </button>
      )}
    </div>
  );
}

/**
 * Die Spielfilter als eine einzige, waagerecht scrollende Zeile.
 *
 * Vorher wuchs die Leiste nach rechts aus dem Fenster heraus und lange
 * Fenstertitel brachen innerhalb ihrer Pille um. Jetzt gilt: eine Zeile, feste
 * Höhe, lange Namen werden gekürzt — und was gar kein Spiel ist, lässt sich
 * mit dem × wegräumen, statt für immer dazustehen.
 */
function GameFilters({
  games,
  untagged,
  total,
  active,
  onSelect,
  onRemove,
}: {
  games: Array<{ name: string; count: number }>;
  untagged: number;
  total: number;
  active: string | null;
  onSelect: (game: string | null) => void;
  onRemove: (game: string) => void;
}) {
  const strip = useRef<HTMLDivElement>(null);
  const [fade, setFade] = useState({ left: false, right: false });
  const [confirming, setConfirming] = useState<string | null>(null);

  const measure = useCallback(() => {
    const el = strip.current;
    if (!el) return;
    const max = el.scrollWidth - el.clientWidth;
    setFade({ left: el.scrollLeft > 2, right: el.scrollLeft < max - 2 });
  }, []);

  useLayoutEffect(measure, [measure, games, untagged]);

  // Das Mausrad kippen: In der Leiste gibt es nichts, was senkrecht scrollen
  // könnte, also soll das Rad sie waagerecht bewegen. Nur wenn sie wirklich
  // übersteht — sonst nähme sie der Galerie grundlos das Scrollen weg.
  useEffect(() => {
    const el = strip.current;
    if (!el) return;
    const onWheel = (event: WheelEvent) => {
      if (event.deltaY === 0 || el.scrollWidth <= el.clientWidth) return;
      event.preventDefault();
      el.scrollLeft += event.deltaY;
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => {
      el.removeEventListener("wheel", onWheel);
      observer.disconnect();
    };
  }, [measure]);

  // Die Rückfrage darf nicht stehen bleiben, wenn man woanders weiterarbeitet.
  useEffect(() => {
    if (!confirming) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setConfirming(null);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [confirming]);

  const edge = (on: boolean) => (on ? "36px" : "0px");
  const mask = `linear-gradient(to right, transparent, #000 ${edge(fade.left)},
    #000 calc(100% - ${edge(fade.right)}), transparent)`;

  return (
    <div
      ref={strip}
      onScroll={measure}
      className="no-scrollbar flex items-center gap-2 overflow-x-auto py-0.5"
      style={{ maskImage: mask, WebkitMaskImage: mask }}
    >
      <Chip active={active === null} onClick={() => onSelect(null)} count={total}>
        Alle
      </Chip>

      {games.map(({ name, count }) =>
        confirming === name ? (
          <ConfirmChip
            key={name}
            name={name}
            onConfirm={() => {
              onRemove(name);
              setConfirming(null);
            }}
            onCancel={() => setConfirming(null)}
          />
        ) : (
          <Chip
            key={name}
            active={active === name}
            count={count}
            onClick={() => onSelect(name)}
            onRemove={() => setConfirming(name)}
          >
            {name}
          </Chip>
        ),
      )}

      {untagged > 0 && (
        <Chip
          active={active === NO_GAME}
          count={untagged}
          onClick={() => onSelect(NO_GAME)}
        >
          Ohne Spiel
        </Chip>
      )}
    </div>
  );
}

/**
 * Eine Filterpille. Der ×-Knopf sitzt fest im Layout und wird nur sichtbar,
 * wenn man die Pille anfasst — täte er das nicht, sprängen beim Überfahren
 * alle folgenden Pillen zur Seite.
 */
function Chip({
  active,
  count,
  onClick,
  onRemove,
  children,
}: {
  active: boolean;
  count: number;
  onClick: () => void;
  onRemove?: () => void;
  children: string;
}) {
  return (
    <div
      className={cn(
        "group/chip flex h-8 shrink-0 items-center rounded-pill border",
        "transition-colors duration-150",
        active
          ? "border-white bg-white text-black"
          : "border-line bg-surface text-ink-muted hover:border-line-strong hover:text-ink",
      )}
    >
      <button
        onClick={onClick}
        title={children}
        className={cn(
          "flex h-full min-w-0 items-center gap-1.5 rounded-pill pl-4 text-[13px] font-medium",
          onRemove ? "pr-1.5" : "pr-4",
        )}
      >
        <span className="max-w-[180px] truncate">{children}</span>
        <span className={cn("tabular-nums", active ? "text-black/45" : "text-ink-faint")}>
          {count}
        </span>
      </button>
      {onRemove && (
        <button
          aria-label={`Filter „${children}" entfernen`}
          title="Filter entfernen — der Spielname wird von diesen Clips gelöst"
          onClick={onRemove}
          className={cn(
            "mr-1 grid h-6 w-6 shrink-0 place-items-center rounded-pill opacity-0 transition",
            "group-hover/chip:opacity-100 focus-visible:opacity-100",
            active ? "hover:bg-black/10 hover:text-black" : "hover:bg-hover hover:text-live",
          )}
        >
          <IconClose className="h-3 w-3" />
        </button>
      )}
    </div>
  );
}

/** Die Rückfrage steht an der Stelle der Pille — kein Dialog über der Seite. */
function ConfirmChip({
  name,
  onConfirm,
  onCancel,
}: {
  name: string;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <div
      className="flex h-8 shrink-0 items-center gap-1 rounded-pill border border-live/40
        bg-live/15 pl-4 text-[13px] font-medium text-live"
    >
      <span className="max-w-[160px] truncate" title={name}>
        {name}
      </span>
      <span className="whitespace-nowrap">entfernen?</span>
      <button
        aria-label="Entfernen bestätigen"
        onClick={onConfirm}
        autoFocus
        className="ml-1 grid h-6 w-6 place-items-center rounded-pill hover:bg-live/25"
      >
        <IconCheck className="h-3.5 w-3.5" />
      </button>
      <button
        aria-label="Abbrechen"
        onClick={onCancel}
        className="mr-1 grid h-6 w-6 place-items-center rounded-pill text-ink-muted hover:bg-hover hover:text-ink"
      >
        <IconClose className="h-3 w-3" />
      </button>
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
