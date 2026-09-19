import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, fileUrl, inTauri } from "@/lib/ipc";
import { clipName, formatDuration, formatSize } from "@/lib/format";
import type { Clip, EngineStatus } from "@/lib/types";
import { cn } from "@/lib/cn";
import { mockClips } from "@/lib/mock";
import {
  IconArrowUpRight,
  IconCamera,
  IconClips,
  IconCopy,
  IconHeart,
  IconPencil,
  IconPlay,
  IconRecord,
  IconScissors,
  IconSearch,
  IconTrash,
} from "@/components/icons";
import { ConfirmDelete } from "@/components/ui/ConfirmDelete";
import { LiveDot } from "@/components/ui/LiveDot";

/** How many clips the strip shows. Beyond that the window in the app is the
 *  better place — this is meant to be read at a glance mid-game. */
const RECENT = 6;

/** The sizes worth exporting for: Discord's limit, and its Nitro limit. */
const DISCORD = [10, 25];

/** How long the way out runs before Rust hides the window — has to match the
 *  `[data-leaving]` rules in console.css. */
const CLOSE_MS = 160;

/** What the dock shows before the core has said anything — in the browser
 *  (`npm run dev`) it stays this way, so the layout can be worked on. */
const RESTING: EngineStatus = {
  bufferActive: true,
  bufferedSeconds: 96,
  bufferBytes: 214_000_000,
  droppedFrames: 0,
  encoder: "nvenc",
  rateControl: "quality",
  fps: 60,
  game: "ARC Raiders",
  recording: false,
  recordingSeconds: 0,
  recordingBytes: 0,
};

type Panel = "clips" | "perf" | null;

/**
 * The console over the game.
 *
 * Rust shows, places and hides the window (`console.rs`); everything here is
 * content. Unlike the banner this window takes the focus, so it also owns
 * Escape: closing goes back through the core, because only the core can hand
 * the focus back to the game.
 */
export function Console() {
  const [status, setStatus] = useState<EngineStatus>(RESTING);
  /** Only for the bar's scale — the configured buffer length. */
  const [bufferLength, setBufferLength] = useState(120);
  /** How big the console is drawn and how much room the task bar wants. Both
   *  come from the core when the window is laid over the screen. */
  const [scale, setScale] = useState(1);
  const [bottomInset, setBottomInset] = useState(0);
  const [clips, setClips] = useState<Clip[]>(inTauri ? [] : mockClips.slice(0, RECENT));
  const [panel, setPanel] = useState<Panel>(null);
  /** The panel that was just closed, kept on screen for as long as its way out
   *  runs. Without it a panel vanished between two frames. */
  const [leavingPanel, setLeavingPanel] = useState<Panel>(null);
  /** The console is on its way out: everything fades, and the core hides the
   *  window once that has played. */
  const [closing, setClosing] = useState(false);
  const [playing, setPlaying] = useState<Clip | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const noteTimer = useRef<number | null>(null);
  const panelTimer = useRef<number | null>(null);
  /** Guards the way out against a second Escape while it runs. */
  const closingRef = useRef(false);

  const say = useCallback((text: string) => {
    setNote(text);
    if (noteTimer.current) window.clearTimeout(noteTimer.current);
    noteTimer.current = window.setTimeout(() => setNote(null), 2600);
  }, []);

  const loadClips = useCallback(async () => {
    if (!inTauri) return;
    try {
      setClips((await api.listClips()).slice(0, RECENT));
    } catch (err) {
      say(String(err));
    }
  }, [say]);

  /** Out of the way first, hidden second. The window itself is hidden by the
   *  core, and that takes effect the same frame — so the whole console has to
   *  have left the screen before the call goes out. */
  const close = useCallback(() => {
    if (!inTauri || closingRef.current) return;
    closingRef.current = true;
    setClosing(true);
    window.setTimeout(() => void api.closeConsole(), CLOSE_MS);
  }, []);

  useEffect(() => {
    if (!inTauri) return;
    void loadClips();
    void api.status().then(setStatus).catch(() => {});
    void api
      .getConfig()
      .then((config) => {
        setBufferLength(Math.max(1, config.buffer.seconds));
        setScale(config.consoleScale);
      })
      .catch(() => {});
    const offs = [
      listen<EngineStatus>("engine-status", (e) => setStatus(e.payload)),
      listen<Clip>("clip-saved", () => void loadClips()),
      // Every opening starts over: whatever was on screen last time is stale,
      // and nobody wants to land in a half-open dialog from an hour ago.
      // Gone from the screen — alt-tabbed away, or the game took the focus
      // back. It comes back as it opens, so everything standing goes now.
      listen("console-closed", () => {
        closingRef.current = false;
        setClosing(false);
        setPanel(null);
        setLeavingPanel(null);
        setPlaying(null);
        setRenaming(null);
        setConfirming(null);
      }),
      listen<{ bottomInset: number; scale: number }>("console-opened", (e) => {
        closingRef.current = false;
        setClosing(false);
        setPanel(null);
        setLeavingPanel(null);
        setPlaying(null);
        setRenaming(null);
        setConfirming(null);
        setScale(e.payload.scale || 1);
        setBottomInset(e.payload.bottomInset);
        void loadClips();
        // The buffer length may have been changed in the app in the meantime;
        // this is the moment it takes hold.
        void api
          .getConfig()
          .then((config) => setBufferLength(Math.max(1, config.buffer.seconds)))
          .catch(() => {});
      }),
    ];
    return () => {
      offs.forEach((off) => void off.then((fn) => fn()));
    };
  }, [loadClips]);

  /** Open something above the dock — a panel, or the player. The window is the
   *  screen and never changes size, so this is nothing but state. */
  const show = useCallback((next: Panel, clip: Clip | null = null) => {
    if (panelTimer.current) window.clearTimeout(panelTimer.current);
    setLeavingPanel(null);
    setPanel(next);
    setPlaying(clip);
  }, []);

  const back = useCallback(() => {
    if (panelTimer.current) window.clearTimeout(panelTimer.current);
    // Held on screen until its way out has run; see `leavingPanel`.
    if (panel) {
      setLeavingPanel(panel);
      panelTimer.current = window.setTimeout(() => setLeavingPanel(null), CLOSE_MS);
    }
    setPanel(null);
    setPlaying(null);
  }, [panel]);

  // Escape backs out one step at a time, and closes the console at the end.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      if (confirming) setConfirming(null);
      else if (renaming) setRenaming(null);
      else if (playing || panel) back();
      else close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [playing, confirming, renaming, panel, close, back]);

  const run = async (what: string, job: () => Promise<unknown>) => {
    setBusy(what);
    try {
      await job();
    } catch (err) {
      say(String(err));
    } finally {
      setBusy(null);
    }
  };

  const saveClip = () =>
    run("clip", async () => {
      await api.saveClip();
      say("Clip gespeichert");
    });
  const record = () =>
    run("rec", async () => {
      const running = await api.toggleRecording();
      say(running ? "Aufnahme läuft" : "Aufnahme gespeichert");
    });
  const shot = () =>
    run("shot", async () => {
      await api.takeScreenshot();
      say("Screenshot gespeichert");
    });

  const buffered = status.bufferActive ? status.bufferedSeconds : 0;
  const share = Math.min(1, buffered / bufferLength);

  /** What is on screen above the dock — the open panel, or the one still
   *  leaving. */
  const sheet = panel ?? leavingPanel;

  return (
    // No veil and no full-screen window: whatever is not the console stays
    // live game. See the head of `console.rs`.
    //
    // A click on the empty room around the console closes what is open — a
    // panel first, then the console itself. `currentTarget` keeps it to the
    // empty part: a click that landed on the dock or a panel has already been
    // dealt with by the thing it hit.
    <div
      onPointerDown={(event) => {
        if (event.target !== event.currentTarget) return;
        if (playing || panel) back();
        else close();
      }}
      className="fixed inset-0 select-none overflow-hidden text-ink"
    >
      {/* The game goes quiet underneath for as long as the console is up.
          Outside `.cb-ui` on purpose: the veil is the screen, not the console,
          and must not be scaled along with it. */}
      <div className="cb-dim" data-leaving={closing} aria-hidden />

      {/* Everything that is operated. The size setting zooms this and nothing
          else — see the head of console.css. `--inset` hangs off it too, so the
          task bar keeps its strip while the veil still covers the whole
          screen. */}
      <div
        className="cb-ui"
        data-leaving={closing}
        style={
          {
            "--cb-scale": scale,
            "--inset": `${bottomInset}px`,
          } as React.CSSProperties
        }
      >
        {/* The player is the one thing that wants room, so it takes the whole
          window — everything else steps aside while it runs. */}
        {playing && (
          <ClipStage
          clip={playing}
          onClose={back}
          onOpenInApp={() => {
            const id = playing.id;
            if (inTauri) void api.showClipInApp(id).catch((err) => say(String(err)));
          }}
          onCopy={() =>
            run("copy", async () => {
              await api.copyClipFile(playing.id);
              say("In der Zwischenablage — Strg+V in Discord");
            })
          }
          />
        )}

        {sheet === "clips" && !playing && (
          <Sheet
          title="Letzte Clips"
          hint="Klick spielt ab · Esc zurück"
          leaving={panel === null}
          >
          {clips.length === 0 ? (
            <p className="px-2 py-8 text-center text-sm text-ink-muted">
              Noch nichts aufgenommen.
            </p>
          ) : (
            <div className="grid grid-cols-3 gap-3">
              {clips.map((clip) => (
                <article key={clip.id} className="relative">
                  <button
                    onClick={() => void show("clips", clip)}
                    className="cb-tile group relative block aspect-video w-full overflow-hidden rounded-inner bg-gradient-to-br from-accent-deep/40 to-black"
                    aria-label={`${clip.screenshot ? "Öffnen" : "Abspielen"}: ${clipName(clip)}`}
                  >
                    {clip.thumbPath && (
                      <img
                        src={`${fileUrl(clip.thumbPath)}?v=${clip.sizeBytes}`}
                        alt=""
                        className="h-full w-full object-cover"
                      />
                    )}
                    {/* What the tile does, said only while the mouse is on it —
                        standing there permanently it would be six marks over
                        six pictures. */}
                    <span className="absolute inset-0 grid place-items-center bg-black/35 opacity-0 transition-opacity duration-150 group-hover:opacity-100">
                      <span className="grid h-10 w-10 place-items-center rounded-pill bg-white/15 backdrop-blur-sm">
                        {clip.screenshot ? (
                          <IconSearch className="h-5 w-5" />
                        ) : (
                          <IconPlay className="h-5 w-5" />
                        )}
                      </span>
                    </span>
                    <span className="absolute inset-x-2 bottom-2 flex items-center justify-between gap-2 text-[11px] font-semibold">
                      <span className="rounded-pill bg-black/60 px-2 py-0.5">
                        {clip.screenshot
                          ? `${clip.width} × ${clip.height}`
                          : formatDuration(clip.durationMs)}
                      </span>
                      <span className="rounded-pill bg-black/60 px-2 py-0.5">
                        {formatSize(clip.sizeBytes)}
                      </span>
                    </span>
                    {clip.recording && (
                      <span className="absolute left-2 top-2 rounded-pill bg-black/60 p-1 text-live">
                        <IconRecord className="h-3 w-3" />
                      </span>
                    )}
                  </button>

                  {renaming === clip.id ? (
                    <input
                      autoFocus
                      id={`rename-${clip.id}`}
                      defaultValue={clipName(clip)}
                      onKeyDown={(e) => {
                        if (e.key !== "Enter") return;
                        const title = e.currentTarget.value.trim();
                        setRenaming(null);
                        void run("rename", async () => {
                          await api.updateClip(clip.id, {
                            title: title || null,
                            description: clip.description,
                            game: clip.game,
                          });
                          await loadClips();
                          say("Umbenannt");
                        });
                      }}
                      onBlur={() => setRenaming(null)}
                      className="mt-2 w-full rounded-inner border border-line bg-base px-2 py-1 text-sm"
                    />
                  ) : (
                    <p className="mt-2 truncate text-sm font-medium">{clipName(clip)}</p>
                  )}

                  <div className="mt-1 flex items-center gap-1">
                    {confirming === clip.id ? (
                      <ConfirmDelete
                        origin="left"
                        onConfirm={() => {
                          setConfirming(null);
                          void run("delete", async () => {
                            await api.deleteClip(clip.id);
                            await loadClips();
                            say("Gelöscht");
                          });
                        }}
                        onCancel={() => setConfirming(null)}
                      />
                    ) : (
                      <>
                        <Tool
                          label="In die Zwischenablage"
                          onClick={() =>
                            run("copy", async () => {
                              await api.copyClipFile(clip.id);
                              say("In der Zwischenablage — Strg+V in Discord");
                            })
                          }
                        >
                          <IconCopy className="h-4 w-4" />
                        </Tool>
                        {!clip.screenshot &&
                          DISCORD.map((mb) => (
                            <Tool
                              key={mb}
                              label={`Auf ${mb} MB verkleinern und kopieren`}
                              busy={busy === `discord-${clip.id}-${mb}`}
                              onClick={() =>
                                run(`discord-${clip.id}-${mb}`, async () => {
                                  await api.exportForDiscord(clip.id, mb);
                                  say(`${mb} MB · in der Zwischenablage`);
                                })
                              }
                            >
                              <span className="text-[11px] font-bold">{mb}</span>
                            </Tool>
                          ))}
                        <Tool
                          label={clip.favorite ? "Favorit entfernen" : "Favorit"}
                          onClick={() =>
                            run("fav", async () => {
                              await api.setClipFavorite(clip.id, !clip.favorite);
                              await loadClips();
                            })
                          }
                        >
                          <IconHeart
                            filled={clip.favorite}
                            className={cn("h-4 w-4", clip.favorite && "text-live")}
                          />
                        </Tool>
                        <Tool label="Umbenennen" onClick={() => setRenaming(clip.id)}>
                          <IconPencil className="h-4 w-4" />
                        </Tool>
                        <Tool label="Löschen" onClick={() => setConfirming(clip.id)} danger>
                          <IconTrash className="h-4 w-4" />
                        </Tool>
                      </>
                    )}
                  </div>
                </article>
              ))}
            </div>
          )}
          </Sheet>
        )}

        {sheet === "perf" && !playing && (
          <Sheet
          title="Leistung"
          hint="Was die Aufnahme gerade kostet"
          leaving={panel === null}
          >
          <div className="grid grid-cols-4 gap-3">
            <Stat label="Bilder/s" value={status.fps.toFixed(0)} hint="gemessen" />
            <Stat
              label="Verworfen"
              value={String(status.droppedFrames)}
              hint={status.droppedFrames > 0 ? "Encoder kommt nicht mit" : "alles drin"}
              bad={status.droppedFrames > 0}
            />
            <Stat
              label="Encoder"
              value={(status.encoder ?? "—").toUpperCase()}
              hint={status.rateControl === "quality" ? "feste Qualität" : "feste Bitrate"}
            />
            <Stat
              label="Puffer"
              value={formatSize(status.bufferBytes)}
              hint={`${Math.round(buffered)} s im Speicher`}
            />
          </div>
          </Sheet>
        )}

        {note && (
          <div className="pointer-events-none absolute bottom-[calc(var(--inset,0px)+11rem)] left-1/2 -translate-x-1/2">
          <div className="cb-note rounded-pill bg-elevated/95 px-4 py-2 text-sm font-medium shadow-[0_12px_32px_rgba(0,0,0,.5)]">
            {note}
          </div>
          </div>
        )}

        {/* The dock. Everything else on screen is a step away from here. */}
        <div
          data-leaving={closing}
          className="cb-dock cb-glass absolute bottom-[calc(var(--inset,0px)+2rem)] left-1/2 flex -translate-x-1/2 items-stretch gap-1.5 rounded-[22px] p-2.5"
        >
          <div className="flex min-w-[200px] flex-col justify-center gap-1.5 px-3">
          <div className="flex items-center gap-2 text-[13px] font-semibold">
            {status.recording ? (
              <>
                <span className="h-2 w-2 shrink-0 rounded-pill bg-live" />
                Aufnahme {formatDuration(status.recordingSeconds * 1000)}
              </>
            ) : status.bufferActive ? (
              <>
                <LiveDot />
                {formatDuration(buffered * 1000)} im Puffer
              </>
            ) : (
              <>
                <span className="h-2 w-2 shrink-0 rounded-pill bg-line-strong" />
                Puffer aus
              </>
            )}
          </div>
          <div className="h-1 overflow-hidden rounded-pill bg-white/10">
            <div
              className="h-full rounded-pill bg-ok transition-[width] duration-500"
              style={{ width: `${share * 100}%` }}
            />
          </div>
          <span className="truncate text-xs text-ink-muted">
            {status.game ?? "Kein Spiel erkannt"}
          </span>
          </div>

          <span className="my-2 w-px bg-white/10" />

          <DockButton
          label="Clip"
          hint={status.bufferActive ? undefined : "Puffer ist aus"}
          disabled={!status.bufferActive || busy === "clip"}
          busy={busy === "clip"}
          onClick={saveClip}
          >
          <IconScissors className="h-6 w-6" />
          </DockButton>
          <DockButton
          label={status.recording ? formatDuration(status.recordingSeconds * 1000) : "Aufnahme"}
          busy={busy === "rec"}
          onClick={record}
          >
          <IconRecord className={cn("h-6 w-6", status.recording && "text-live")} />
          </DockButton>
          <DockButton label="Screenshot" busy={busy === "shot"} onClick={shot}>
          <IconCamera className="h-6 w-6" />
          </DockButton>

          <span className="my-2 w-px bg-white/10" />

          <DockButton
          label="Clips"
          active={panel === "clips"}
          onClick={() => (panel === "clips" ? back() : void show("clips"))}
          >
          <IconClips className="h-6 w-6" />
          </DockButton>
          <DockButton
          label="Leistung"
          active={panel === "perf"}
          onClick={() => (panel === "perf" ? back() : void show("perf"))}
          >
          <span className="text-[15px] font-bold tabular-nums">{status.fps.toFixed(0)}</span>
          </DockButton>
        </div>

          <p className="pointer-events-none absolute bottom-[calc(var(--inset,0px)+0.5rem)] left-1/2 -translate-x-1/2 text-[11px] text-ink-faint">
          Esc schließt · das Spiel läuft weiter · die Konsole ist in keinem Clip zu sehen
          </p>
      </div>
    </div>
  );
}

/** A panel above the dock. Same frame for clips and for the numbers. */
function Sheet({
  title,
  hint,
  leaving,
  children,
}: {
  title: string;
  hint: string;
  leaving?: boolean;
  children: React.ReactNode;
}) {
  return (
    <div className="absolute bottom-[calc(var(--inset,0px)+10rem)] left-1/2 w-[min(1100px,80vw)] -translate-x-1/2">
      <section className="cb-sheet cb-glass rounded-card p-5" data-leaving={leaving}>
        <header className="mb-3 flex items-baseline justify-between gap-4">
          <h2 className="text-[13px] font-bold uppercase tracking-[0.1em] text-ink-muted">
            {title}
          </h2>
          <span className="text-xs text-ink-faint">{hint}</span>
        </header>
        {children}
      </section>
    </div>
  );
}

/**
 * The clip itself, played right here — the whole point of the gallery.
 *
 * Built like the app's own player: the picture on a dark stage, its name and
 * the facts underneath, actions on the right. "In der App öffnen" is the way
 * out of the overlay for everything this small window cannot do — trimming,
 * the track mix, the export dialog.
 */
function ClipStage({
  clip,
  onClose,
  onOpenInApp,
  onCopy,
}: {
  clip: Clip;
  onClose: () => void;
  onOpenInApp: () => void;
  onCopy: () => void;
}) {
  return (
    <div
      onPointerDown={(event) => {
        // The player fills the window, so the room around it is its own to
        // watch: a click beside the picture means "back".
        if (event.target === event.currentTarget) onClose();
      }}
      className="absolute inset-x-0 top-0 bottom-[calc(var(--inset,0px)+12rem)] grid place-items-center p-6"
    >
      <div className="cb-glass w-full max-w-[1180px] overflow-hidden rounded-card">
        <div className="grid place-items-center bg-black/70 p-2">
          {clip.screenshot ? (
            <img
              src={fileUrl(clip.path)}
              alt={clipName(clip)}
              className="max-h-[min(58vh,540px)] w-auto max-w-full rounded-inner object-contain"
            />
          ) : (
            <video
              src={fileUrl(clip.path)}
              controls
              autoPlay
              className="max-h-[min(58vh,540px)] w-full rounded-inner bg-black"
            />
          )}
        </div>

        <div className="flex flex-wrap items-center gap-x-5 gap-y-3 px-5 py-4">
          <div className="min-w-0 flex-1">
            <p className="truncate text-[15px] font-semibold">{clipName(clip)}</p>
            <p className="mt-0.5 truncate text-xs text-ink-muted">
              {[
                clip.game,
                clip.screenshot
                  ? `${clip.width} × ${clip.height}`
                  : formatDuration(clip.durationMs),
                formatSize(clip.sizeBytes),
              ]
                .filter(Boolean)
                .join(" · ")}
            </p>
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={onCopy}
              className="cb-btn inline-flex items-center gap-2 rounded-pill border border-line bg-elevated px-4 py-2 text-[13px] font-semibold hover:border-line-strong hover:bg-hover"
            >
              <IconCopy className="h-4 w-4" />
              Kopieren
            </button>
            <button
              onClick={onOpenInApp}
              className="cb-btn inline-flex items-center gap-2 rounded-pill bg-accent px-4 py-2 text-[13px] font-semibold text-white hover:bg-accent-bright"
            >
              <IconArrowUpRight className="h-4 w-4" />
              In der App öffnen
            </button>
            <button
              onClick={onClose}
              className="cb-btn rounded-pill border border-line bg-elevated px-4 py-2 text-[13px] font-semibold hover:border-line-strong hover:bg-hover"
            >
              Zurück
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function DockButton({
  children,
  label,
  hint,
  onClick,
  active,
  disabled,
  busy,
}: {
  children: React.ReactNode;
  label: string;
  hint?: string;
  onClick: () => void;
  active?: boolean;
  disabled?: boolean;
  busy?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled || busy}
      title={hint ?? label}
      aria-pressed={active}
      className={cn(
        // `relative`: the lit button's underline is drawn on ::after.
        "cb-dock-btn relative flex w-[84px] flex-col items-center gap-1.5 rounded-inner px-2 pb-2 pt-3 text-[12px] font-semibold",
        (disabled || busy) && "cursor-default opacity-45",
      )}
    >
      {/* Wrapped so the hover rule has one element to grow — the icon itself is
          passed in and may be an svg or a number. */}
      <span className="grid h-6 place-items-center">{children}</span>
      {/* The label keeps its place while a job runs: swapping it for "…" made
          the whole dock jump a line every time something was clicked. */}
      <span className="relative truncate">
        <span className={cn(busy && "invisible")}>{label}</span>
        {busy && <span className="absolute inset-0 grid place-items-center">…</span>}
      </span>
    </button>
  );
}

function Tool({
  children,
  label,
  onClick,
  danger,
  busy,
}: {
  children: React.ReactNode;
  label: string;
  onClick: () => void;
  danger?: boolean;
  busy?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      title={label}
      aria-label={label}
      disabled={busy}
      className={cn(
        "cb-tool grid h-7 w-7 place-items-center rounded-inner border border-line bg-elevated text-ink-muted",
        "hover:border-line-strong hover:bg-hover hover:text-ink",
        danger && "hover:border-live hover:bg-live/15 hover:text-live",
        busy && "cursor-default opacity-50",
      )}
    >
      {busy ? "…" : children}
    </button>
  );
}

function Stat({
  label,
  value,
  hint,
  bad,
}: {
  label: string;
  value: string;
  hint: string;
  bad?: boolean;
}) {
  return (
    <div className="rounded-inner border border-line bg-black/25 p-3">
      <p className="text-[11px] font-bold uppercase tracking-[0.1em] text-ink-muted">{label}</p>
      <p className={cn("mt-1 text-2xl font-bold tabular-nums", bad && "text-live")}>{value}</p>
      <p className="text-xs text-ink-faint">{hint}</p>
    </div>
  );
}
