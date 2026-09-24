import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, fileUrl, inTauri } from "@/lib/ipc";
import { clipName, formatDuration, formatSize } from "@/lib/format";
import type { AudioSource, Clip, ConsoleStyle, EngineStatus } from "@/lib/types";
import { cn } from "@/lib/cn";
import { mockClips, mockConfig } from "@/lib/mock";
import {
  IconArrowUpRight,
  IconAudio,
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
import {
  PlayButton,
  Scrubber,
  Volume,
  clock,
  jump,
} from "@/components/ui/PlayerControls";
import { AudioPanel, useLevelBars } from "./Audio";
import { TREND_SECONDS, TrendChart, useTrend } from "./Trend";
import { TROUBLE_LINES, TroubleStrip, troubles, useAudioTrouble } from "./Trouble";

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
  screenFallback: null,
};

type Panel = "clips" | "audio" | "perf" | null;

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
  /** Die eingestellte Bildrate — die gemessene allein sagt nichts. */
  const [targetFps, setTargetFps] = useState(60);
  /** How big the console is drawn and how much room the task bar wants. Both
   *  come from the core when the window is laid over the screen. */
  const [scale, setScale] = useState(1);
  const [bottomInset, setBottomInset] = useState(0);
  const [clips, setClips] = useState<Clip[]>(inTauri ? [] : mockClips.slice(0, RECENT));
  /** Die eingestellten Tonquellen — allein für den Warnstreifen. Der Kern
   *  meldet Fehler und Auffälligkeiten je Quellen-Id, und einen Namen dazu hat
   *  nur die Konfiguration. */
  const [sources, setSources] = useState<AudioSource[]>(inTauri ? [] : mockConfig.sources);
  const [panel, setPanel] = useState<Panel>(null);
  /** Zählt jedes Öffnen. Das Fenster wird nur versteckt, die Seite bleibt
   *  geladen — ohne diesen Schlüssel liefe das Licht um das Dock (`cb-dock`)
   *  genau einmal pro Programmstart statt bei jedem Öffnen. */
  const [opened, setOpened] = useState(0);
  /** The panel that was just closed, kept on screen for as long as its way out
   *  runs. Without it a panel vanished between two frames. */
  const [leavingPanel, setLeavingPanel] = useState<Panel>(null);
  /** The console is on its way out: everything fades, and the core hides the
   *  window once that has played.
   *
   *  Startet in der App auf `true`, also „nicht auf dem Schirm". Das Fenster
   *  lebt zwischen den Öffnungen weiter, und was darin gezeichnet steht,
   *  während es versteckt ist, ist genau das, was beim nächsten `show()` für
   *  die ersten Bilder zu sehen ist — bevor `console-opened` überhaupt
   *  ankommt. Stand die Seite dabei fertig gezeichnet da, sah man das alte
   *  Dock kurz stehen und dann auffahren: das Zucken beim Öffnen. Im Browser
   *  (`npm run dev`) gibt es kein Fenster, das versteckt würde — dort steht
   *  die Konsole von Anfang an da. */
  const [closing, setClosing] = useState(inTauri);
  /** Der Clip im Player, als Id. Als Objekt wäre er nach jedem `loadClips()`
   *  veraltet — ein gerade geänderter Name oder Favorit stünde dann im Player
   *  noch alt da. */
  const [playingId, setPlayingId] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const noteTimer = useRef<number | null>(null);
  const panelTimer = useRef<number | null>(null);
  /** Guards the way out against a second Escape while it runs. */
  const closingRef = useRef(false);
  /** Der ausstehende Aufruf am Ende des Wegs hinaus, zum Abbrechen. */
  const closeTimer = useRef<number | null>(null);
  /** Ob die Konsole in Bildschirmaufnahmen zu sehen ist — nur für den Satz
   *  unter dem Dock, der sonst etwas Falsches behauptet. */
  const [inCapture, setInCapture] = useState(false);
  /** Der violette Schein in den unteren Bildschirmecken. Kommt in der App
   *  ausschließlich aus `console-opened` — der Kern legt ihn dem Ereignis bei,
   *  mit dem die Konsole aufgeht, damit er im selben Bild steht wie alles
   *  andere. Im Browser (`npm run dev`) gibt es keinen Kern, dort ist er an,
   *  damit man ihn beim Bauen sieht. */
  const [glow, setGlow] = useState(!inTauri);
  /** Die Gestalt des Docks. Kommt denselben Weg wie `glow` und aus demselben
   *  Grund: das Fenster steht schon, wenn die Seite es erfährt, und ein Dock,
   *  das sich nach dem Aufgehen noch umbaut, hätte man gesehen. */
  const [style, setStyle] = useState<ConsoleStyle>("dock");

  /** Der Verlauf der letzten Minute (k07) und was der Kern gerade an Meldungen
   *  schickt (k02). Beide sammeln auch dann weiter, wenn die Konsole zu ist —
   *  das Fenster lebt ja, und die Ereignisse gehen an jedes. Genau darauf kommt
   *  es an: man macht die Konsole auf, *weil* eben etwas geruckelt hat, und
   *  findet den Einbruch dann noch vor. */
  const trend = useTrend(targetFps);
  const trendPush = trend.push;
  const { errors: sourceErrors, warnings: sourceWarnings } = useAudioTrouble();
  /** Die Pegelbalken des Ton-Panels. Sie werden an React vorbei gezeichnet —
   *  warum, steht in `Audio.tsx`. */
  const levelBars = useLevelBars();

  /** Der laufende Clip, wie er zuletzt aus der Liste kam. Die Liste hält nur
   *  die letzten sechs: wird während des Zusehens einer gespeichert, fällt der
   *  sechste heraus — und ohne diesen Rückhalt fiele der Player mit ihm zu. */
  const kept = useRef<Clip | null>(null);
  const fresh = playingId ? (clips.find((c) => c.id === playingId) ?? null) : null;
  useEffect(() => {
    if (fresh) kept.current = fresh;
  }, [fresh]);
  const playing = playingId ? (fresh ?? kept.current) : null;

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
    closeTimer.current = window.setTimeout(() => {
      closeTimer.current = null;
      void api.closeConsole();
    }, CLOSE_MS);
  }, []);

  useEffect(() => {
    if (!inTauri) return;
    void loadClips();
    void api.status().then(setStatus).catch(() => {});
    void api
      .getConfig()
      .then((config) => {
        setBufferLength(Math.max(1, config.buffer.seconds));
        setTargetFps(config.recording.fps);
        setScale(config.consoleScale);
        setInCapture(config.consoleInCapture);
        setSources(config.sources);
      })
      .catch(() => {});
    const offs = [
      listen<EngineStatus>("engine-status", (e) => {
        setStatus(e.payload);
        // Im selben Zuhörer, damit beide Änderungen in einem Rendern landen.
        trendPush(e.payload);
      }),
      listen<Clip>("clip-saved", () => void loadClips()),
      // Gone from the screen — alt-tabbed away, or the game took the focus
      // back. It comes back as it opens, so everything standing goes now.
      listen("console-closed", () => {
        // Der Weg hinaus ist überholt, das Fenster ist schon weg. Bricht man
        // den ausstehenden Aufruf nicht ab, läuft er gleich noch los und zieht
        // über `restore_foreground()` den Vordergrund von dem weg, wohin
        // gerade getabbt wurde.
        if (closeTimer.current !== null) {
          window.clearTimeout(closeTimer.current);
          closeTimer.current = null;
        }
        closingRef.current = false;
        // Auf `true`, nie auf `false`. Auf `false` ließ es die
        // Eingangs-Animation auf einem Fenster wieder anlaufen, das gerade
        // verschwindet — mitten in der Ausgangs-Animation sprang das Dock
        // zurück, und das war das Zucken beim Raustabben.
        //
        // Stehenlassen war aber auch falsch: wer weggetabbt ist, hat nie
        // `closing` gesetzt, und die Seite blieb fertig gezeichnet hinter
        // einem versteckten Fenster liegen. Beim nächsten Öffnen stand genau
        // dieses alte Bild ein paar Frames lang da. Hier ist das Fenster schon
        // weg (der Kern versteckt es, bevor er das hier sendet), die Blende
        // sieht also niemand — sie hinterlässt nur eine leere Seite für das
        // nächste Mal.
        setClosing(true);
        setPanel(null);
        setLeavingPanel(null);
        setPlayingId(null);
        setRenaming(null);
        setConfirming(null);
      }),
      listen<{ bottomInset: number; scale: number; glow: boolean; style: ConsoleStyle }>(
        "console-opened",
        (e) => {
          closingRef.current = false;
          setClosing(false);
          setOpened((n) => n + 1);
          setPanel(null);
          setLeavingPanel(null);
          setPlayingId(null);
          setRenaming(null);
          setConfirming(null);
          setScale(e.payload.scale || 1);
          setBottomInset(e.payload.bottomInset);
          // Der Schein kommt mit dem Ereignis, nicht aus dem `getConfig()`
          // darunter: der ist ein Aufruf über die Brücke und kommt erst ein paar
          // Bilder später zurück. Wurde er zwischendurch abgeschaltet, während
          // die Konsole zu war, sah man ihn genau so lange noch einmal
          // aufleuchten. Hier steht er im selben Rutsch wie `closing` — eine
          // Zeichnung, kein Nachziehen.
          setGlow(e.payload.glow);
          setStyle(e.payload.style);
          void loadClips();
          // The buffer length may have been changed in the app in the meantime;
          // this is the moment it takes hold.
          void api
            .getConfig()
            .then((config) => {
              setBufferLength(Math.max(1, config.buffer.seconds));
              setTargetFps(config.recording.fps);
              setInCapture(config.consoleInCapture);
              setSources(config.sources);
            })
            .catch(() => {});
        },
      ),
    ];
    return () => {
      offs.forEach((off) => void off.then((fn) => fn()));
    };
  }, [loadClips, trendPush]);

  /** Open something above the dock — a panel, or the player. The window is the
   *  screen and never changes size, so this is nothing but state. */
  const show = useCallback(
    (next: Panel, clipId: string | null = null) => {
      if (panelTimer.current) window.clearTimeout(panelTimer.current);
      // Von einem Panel direkt zum anderen: das alte bleibt stehen, solange
      // sein Weg hinaus läuft, und die beiden überlappen sich dabei. Vorher
      // wurde es in demselben Bild weggenommen, in dem das neue aufging — ein
      // harter Tausch, bei dem für einen Moment gar nichts dastand.
      //
      // Nicht beim Öffnen eines Clips aus der Clip-Liste heraus: da ist `next`
      // dasselbe Panel, und stehen bleibt ohnehin nichts, weil der Player an
      // die Stelle beider tritt.
      const crossing = panel !== null && next !== null && panel !== next;
      setLeavingPanel(crossing ? panel : null);
      if (crossing) {
        panelTimer.current = window.setTimeout(() => setLeavingPanel(null), CLOSE_MS);
      }
      setPanel(next);
      setPlayingId(clipId);
    },
    [panel],
  );

  const back = useCallback(() => {
    if (panelTimer.current) window.clearTimeout(panelTimer.current);
    // Held on screen until its way out has run; see `leavingPanel`.
    if (panel) {
      setLeavingPanel(panel);
      panelTimer.current = window.setTimeout(() => setLeavingPanel(null), CLOSE_MS);
    }
    setPanel(null);
    setPlayingId(null);
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

  /** Eine Tonquelle ändern: lauter, leiser, stumm.
   *
   *  Die Anzeige folgt sofort, der Kern mit einer kurzen Bremse. Der Grund
   *  steht in `state.rs`: `upsert_source` geht über `replace_config`, und das
   *  richtet die Tonquellen neu aus **und** schreibt die Konfiguration auf die
   *  Platte. Am Regler gezogen wären das zwanzig Plattenschreiber die Sekunde,
   *  über einem laufenden Spiel. Das Stummschalten ist ein einzelner Klick und
   *  geht ohne Bremse durch — dort soll nichts hinterherhinken.
   *
   *  Je Quelle eine eigene Bremse, nicht eine gemeinsame: sonst verschluckte
   *  ein Griff an der zweiten Quelle den noch ausstehenden Stand der ersten.
   *
   *  Was der Kern zurückgibt, wird bewusst **nicht** übernommen. Solange die
   *  Konsole offen ist, kommt jede Änderung von hier, und beim nächsten Öffnen
   *  liest sie die Konfiguration ohnehin frisch. Das Zurückschreiben hätte nur
   *  die Chance eröffnet, dass ein verspätetes Ergebnis einen neueren Reglerweg
   *  wieder einkassiert. */
  const sourceTimers = useRef(new Map<string, number>());
  const changeSource = useCallback(
    (next: AudioSource, now = false) => {
      setSources((prev) => prev.map((s) => (s.id === next.id ? next : s)));
      if (!inTauri) return;
      const timers = sourceTimers.current;
      const running = timers.get(next.id);
      if (running) window.clearTimeout(running);
      const send = () => {
        timers.delete(next.id);
        void api.updateAudioSource(next).catch((err) => say(String(err)));
      };
      if (now) send();
      else timers.set(next.id, window.setTimeout(send, 150));
    },
    [say],
  );

  const buffered = status.bufferActive ? status.bufferedSeconds : 0;
  const share = Math.min(1, buffered / bufferLength);

  /** Was gerade schiefläuft, für den Streifen über dem Dock. */
  const trouble = troubles({
    status,
    sources,
    errors: sourceErrors,
    warnings: sourceWarnings,
    droppedRecently: trend.dropped,
  });

  /** Die Zeile unter der Bildrate. Sie sagt nicht mehr nur, wie es *jetzt*
   *  steht, sondern wann es zuletzt nicht so stand — dafür ist der Verlauf da. */
  const fpsHint = () => {
    if (!status.bufferActive) return "Puffer aus";
    if (trend.dipAgo === 0) return `von ${targetFps} · bricht gerade ein`;
    if (trend.dipAgo !== null) return `von ${targetFps} · Einbruch vor ${trend.dipAgo} s`;
    // „gemessen" allein stand hier und las sich wie „so schnell läuft deine
    // Aufnahme". Gemeint ist etwas anderes: wie oft sich das Bild wirklich
    // geändert hat. Die Aufnahme läuft immer auf der eingestellten Rate — steht
    // das Bild still, wiederholt die Pipeline das letzte, und genau die fehlen.
    return status.fps >= targetFps - 1
      ? `von ${targetFps} · volles Bild`
      : `von ${targetFps} · Bild stand still`;
  };

  /** Ein Panel, wie es über dem Dock steht.
   *
   *  Als Funktion und nicht als zwei Blöcke im Baum, weil beim Wechsel zwei
   *  davon gleichzeitig dastehen — das gehende und das kommende. Welches oben
   *  liegt, entscheidet dann die Reihenfolge im DOM, und die muss dieselbe
   *  sein, egal in welche Richtung gewechselt wird. Als feste Blöcke hätte
   *  der Weg von den Clips zur Leistung anders ausgesehen als der Rückweg,
   *  weil dort mal das eine und mal das andere weiter unten stand. */
  const sheetFor = (which: Exclude<Panel, null>, leaving: boolean) =>
    which === "clips" ? (
      <Sheet title="Letzte Clips" hint="Klick spielt ab · Esc zurück" leaving={leaving}>
        {clips.length === 0 ? (
          <p className="px-2 py-8 text-center text-sm text-ink-muted">
            Noch nichts aufgenommen.
          </p>
        ) : (
          <div className="grid grid-cols-3 gap-3">
            {clips.map((clip) => (
              <article key={clip.id} className="relative">
                <button
                  onClick={() => void show("clips", clip.id)}
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
    ) : which === "audio" ? (
      <Sheet
        title="Ton"
        hint="Gilt ab dem nächsten Clip"
        leaving={leaving}
      >
        <AudioPanel sources={sources} bars={levelBars} onChange={changeSource} />
      </Sheet>
    ) : (
      <Sheet
        title="Leistung"
        hint={`Die letzten ${TREND_SECONDS} Sekunden`}
        leaving={leaving}
      >
        {/* Der Verlauf steht über den Zahlen, nicht darunter: er beantwortet
            die Frage, mit der man das Panel aufmacht — war da eben was? Die
            Kacheln sagen dann, was es war. */}
        {trend.samples.length > 1 ? (
          <figure className="cb-trend-box mb-3">
            <TrendChart samples={trend.samples} targetFps={targetFps} />
            <figcaption className="mt-1 flex justify-between text-[11px] text-ink-faint">
              <span>vor {TREND_SECONDS} s</span>
              <span>{targetFps} Bilder/s · rot: verworfen</span>
              <span>jetzt</span>
            </figcaption>
          </figure>
        ) : (
          <p className="mb-3 text-xs text-ink-faint">
            Der Verlauf füllt sich — eine Messung je Sekunde.
          </p>
        )}
        <div className="grid grid-cols-4 gap-3">
          <Stat label="Bilder/s" value={status.fps.toFixed(0)} hint={fpsHint()} />
          <Stat
            label="Verworfen"
            // Der Zählerstand aus dem Status steht seit dem Start der Pipeline
            // da und wächst nur. Hier zählt, was gerade fehlt — der Rest steht
            // in der Zeile darunter.
            value={String(trend.dropped)}
            hint={
              trend.dropped > 0
                ? `in der letzten Minute · ${status.droppedFrames} gesamt`
                : status.droppedFrames > 0
                  ? `nichts zuletzt · ${status.droppedFrames} gesamt`
                  : "alles drin"
            }
            bad={trend.dropped > 0}
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
    );

  /** Was über dem Dock steht. Beim Wechsel sind es zwei: das gehende Panel
   *  zuerst, das kommende danach. */
  const stack: { which: Exclude<Panel, null>; leaving: boolean }[] = [];
  if (leavingPanel && leavingPanel !== panel) {
    stack.push({ which: leavingPanel, leaving: true });
  }
  if (panel) stack.push({ which: panel, leaving: false });

  return (
    // Kein Schleier über dem Spiel: was nicht die Konsole ist, bleibt
    // unverändertes Spiel. Dock und Panels tragen ihre Lesbarkeit selbst, als
    // fast deckende dunkle Fläche — siehe `.cb-dock` und `.cb-glass`.
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
      {/* Der Schein aus den unteren Ecken. Steht mit Absicht vor und außerhalb
          von `.cb-ui`: dahinter, weil er hinter allem Bedienbaren liegen soll,
          und außerhalb, weil `.cb-ui` die Größeneinstellung als `zoom` trägt —
          darin gerechnet wüchse der Verlauf bei Größe 1,6 aus dem Bild heraus,
          statt in den Ecken des Bildschirms zu sitzen, die er meint. */}
      {glow && <div className="cb-glow" data-leaving={closing} />}
      {/* Everything that is operated. The size setting zooms this and nothing
          else — see the head of console.css. `--inset` hangs off it too, so the
          dock keeps clear of the task bar. */}
      <div
        className="cb-ui"
        data-style={style}
        // Nur für den Ring: der steht mitten im Bild, und der Player nimmt
        // sich das ganze Fenster. Die beiden lägen übereinander, also tritt
        // der Ring zurück, solange ein Clip läuft. Die Leiste unten hat das
        // Problem nicht, der Player endet über ihr.
        data-playing={playing ? "true" : "false"}
        // Ebenfalls nur für den Ring: der steht in der Mitte, bis etwas
        // aufgeht, und rückt dann zur Seite, um dem Panel Platz zu machen.
        // `panel` und nicht `panel ?? leavingPanel`: beim Zumachen soll er
        // gleich zurückwandern, während das Panel noch abblendet — zuerst
        // Platz schaffen, dann hinlegen, und umgekehrt.
        data-panel={panel ? "open" : "closed"}
        // Wie viele Zeilen der Warnstreifen über dem Dock gerade belegt. Die
        // Notiz („Clip gespeichert") steht darüber und rückt um genau so viel
        // nach oben — siehe `--warn-lift` in console.css.
        data-warn={Math.min(trouble.length, TROUBLE_LINES)}
        data-leaving={closing}
        style={
          {
            "--cb-scale": String(scale),
            "--inset": `${bottomInset}px`,
          } as React.CSSProperties
        }
      >
        {/* The player is the one thing that wants room, so it takes the whole
            window — everything else steps aside while it runs. */}
        {playing && (
          <ClipStage
            clip={playing}
            renaming={renaming === playing.id}
            onRenaming={(on) => setRenaming(on ? playing.id : null)}
            onRename={(title) => {
              setRenaming(null);
              void run("rename", async () => {
                await api.updateClip(playing.id, {
                  title: title || null,
                  description: playing.description,
                  game: playing.game,
                });
                await loadClips();
                say("Umbenannt");
              });
            }}
            onClose={back}
            onOpenInApp={(at) => {
              const id = playing.id;
              if (inTauri) void api.showClipInApp(id, at).catch((err) => say(String(err)));
            }}
            onCopy={() =>
              run("copy", async () => {
                await api.copyClipFile(playing.id);
                say("In der Zwischenablage — Strg+V in Discord");
              })
            }
          />
        )}

        {/* Beim Wechsel stehen beide übereinander: das gehende zuerst, damit
            das kommende darüber liegt. Der Schlüssel ist das Panel selbst —
            ohne ihn nähme React denselben Kasten für beide und die
            Animation liefe gar nicht erst an. */}
        {!playing &&
          stack.map(({ which, leaving }) => (
            <Fragment key={which}>{sheetFor(which, leaving)}</Fragment>
          ))}

        {note && (
          // Über dem Dock, wo der Klick war — außer der Player steht da, denn
          // dann läge die Notiz genau auf der Schaltfläche, die sie
          // beantwortet. Dann oben.
          <div
            className={cn(
              "cb-note-slot pointer-events-none absolute left-1/2 -translate-x-1/2",
              playing
                ? "top-8"
                : "bottom-[calc(var(--inset,0px)+11rem+var(--warn-lift,0px))]",
            )}
          >
            <div className="cb-note rounded-pill bg-elevated/95 px-4 py-2 text-sm font-medium shadow-[0_12px_32px_rgba(0,0,0,.5)]">
              {note}
            </div>
          </div>
        )}

        {/* The dock. Everything else on screen is a step away from here.

            Zwei Kästen, nicht einer: die Auffahrt sitzt auf dem äußeren, die
            Abfahrt auf dem Dock selbst — warum, steht bei `.cb-rise` in
            console.css. Der Schlüssel gehört nach außen, damit beide
            Animationen bei jedem Öffnen von vorn anfangen. */}
        <div
          key={opened}
          data-leaving={closing}
          className="cb-rise absolute bottom-[calc(var(--inset,0px)+2rem)] left-1/2 -translate-x-1/2"
        >
          {/* Der Warnstreifen sitzt im selben Kasten wie das Dock und liegt
              über ihm (`bottom: 100%`). Damit geht er jeden Weg mit, den das
              Dock geht — auch den des Rings, der mitten im Bild steht und zur
              Seite rückt. Eine eigene Stelle im Fenster hätte für jeden der
              drei Stile eine eigene Regel gebraucht. */}
          <TroubleStrip list={trouble} />
          <div
            data-leaving={closing}
            className="cb-dock cb-glass flex items-stretch gap-1.5 rounded-[22px] p-2.5"
          >
            <div className="cb-status flex min-w-[200px] flex-col justify-center gap-1.5 px-3">
              <div className="cb-status-line flex items-center gap-2 text-[13px] font-semibold">
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
              <div className="cb-bar h-1 overflow-hidden rounded-pill bg-black/35">
                <div
                  className="h-full rounded-pill bg-accent-bright transition-[width] duration-500"
                  style={{ width: `${share * 100}%` }}
                />
              </div>
              <span className="truncate text-xs text-ink-muted">
                {status.game ?? "Kein Spiel erkannt"}
              </span>
            </div>

            <span className="cb-sep my-2 w-px bg-accent-bright/35" />

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
              label={
                status.recording ? formatDuration(status.recordingSeconds * 1000) : "Aufnahme"
              }
              busy={busy === "rec"}
              onClick={record}
            >
              <IconRecord className={cn("h-6 w-6", status.recording && "text-live")} />
            </DockButton>
            <DockButton label="Screenshot" busy={busy === "shot"} onClick={shot}>
              <IconCamera className="h-6 w-6" />
            </DockButton>

            <span className="cb-sep my-2 w-px bg-accent-bright/35" />

            <DockButton
              label="Clips"
              active={panel === "clips"}
              onClick={() => (panel === "clips" ? back() : void show("clips"))}
            >
              <IconClips className="h-6 w-6" />
            </DockButton>
            <DockButton
              label="Ton"
              active={panel === "audio"}
              onClick={() => (panel === "audio" ? back() : void show("audio"))}
            >
              <IconAudio className="h-6 w-6" />
            </DockButton>
            <DockButton
              label="Leistung"
              active={panel === "perf"}
              onClick={() => (panel === "perf" ? back() : void show("perf"))}
            >
              <span className="text-[15px] font-bold tabular-nums">{status.fps.toFixed(0)}</span>
            </DockButton>
          </div>
        </div>

        <p
          data-leaving={closing}
          className="cb-dock-line pointer-events-none absolute bottom-[calc(var(--inset,0px)+0.5rem)] left-1/2 -translate-x-1/2 text-[11px] text-ink-faint"
        >
          Esc schließt · das Spiel läuft weiter ·{" "}
          {inCapture
            ? "in Aufnahmen und im Stream sichtbar"
            : "die Konsole ist in keinem Clip zu sehen"}
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
    // Prozent, nicht `vw`: `.cb-ui` trägt `zoom`, und Chromium multipliziert
    // Viewport-Einheiten damit — bei Größe 1,6 waren aus 80vw 128 % der
    // Bildschirmbreite geworden, und die Clips standen neben dem Bild. Ein
    // Prozentsatz misst gegen das Elternteil und ist vom Zoom unberührt; die
    // Obergrenze in Pixeln darf mitwachsen, sie deckelt ja nur.
    // Derselbe Schnitt wie beim Dock: die Auffahrt auf dem äußeren Kasten, die
    // Abfahrt auf dem Panel selbst — siehe `.cb-rise` in console.css. Sonst
    // ersetzt die eine Animation die andere, und ein Panel, das mitten im
    // Aufgehen geschlossen wird, springt erst an sein Ende.
    <div
      data-leaving={leaving}
      className="cb-sheet-pos absolute bottom-[calc(var(--inset,0px)+10rem)] left-1/2 w-[80%] max-w-[1100px] -translate-x-1/2"
    >
      <section
        className="cb-sheet cb-glass cb-cap overflow-y-auto rounded-card p-5"
        data-leaving={leaving}
      >
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
 * Der Clip selbst, hier abgespielt — wofür die Galerie da ist.
 *
 * Gebaut wie der Player der App, bis hin zur Leiste darunter: die kommt aus
 * `ui/PlayerControls` und ist dieselbe wie im Fenster, nur ohne Ausschnitt.
 * Die eingebauten `controls` des Browsers waren das einzige Stück Konsole, das
 * nicht nach ClippiBoy aussah.
 *
 * „In der App öffnen" ist der Weg hinaus für alles, was dieses kleine Fenster
 * nicht kann — Schneiden, die Tonspuren, der Export-Dialog —, und nimmt die
 * Stelle mit, an der hier gerade geschaut wurde.
 */
function ClipStage({
  clip,
  renaming,
  onRenaming,
  onRename,
  onClose,
  onOpenInApp,
  onCopy,
}: {
  clip: Clip;
  renaming: boolean;
  onRenaming: (on: boolean) => void;
  onRename: (title: string) => void;
  onClose: () => void;
  /** Bekommt die Sekunde, an der der Clip gerade steht. */
  onOpenInApp: (at: number) => void;
  onCopy: () => void;
}) {
  const video = useRef<HTMLVideoElement>(null);
  const [playing, setPlaying] = useState(false);
  const [time, setTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [volume, setVolume] = useState(1);
  const [muted, setMuted] = useState(false);

  // Während des Abspielens wird die Stelle an React vorbei gezeichnet: die
  // Bildschleife schreibt Balken, Griff und Uhr direkt in den DOM. Ein
  // `setTime` pro Bild wären sechzig Renderings die Sekunde, und WebView2
  // rendert React und das Video auf demselben Faden — im Fenster der App hat
  // genau das Videobilder gekostet (siehe `ClipPlayer`).
  const fill = useRef<HTMLDivElement>(null);
  const knob = useRef<HTMLDivElement>(null);
  const clockLabel = useRef<HTMLSpanElement>(null);

  const paint = useCallback((seconds: number, total: number) => {
    const ratio = total > 0 ? Math.min(1, Math.max(0, seconds / total)) : 0;
    const percent = `${ratio * 100}%`;
    if (fill.current) fill.current.style.width = percent;
    if (knob.current) knob.current.style.left = percent;
    if (clockLabel.current) clockLabel.current.textContent = clock(seconds);
  }, []);

  // `timeupdate` kommt nur etwa viermal die Sekunde — der Balken würde sichtbar
  // springen.
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    const tick = () => {
      const element = video.current;
      if (element) paint(element.currentTime, element.duration);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing, paint]);

  // Die Lautstärke gehört dem Element: hier läuft nur die eine Tonspur, anders
  // als im Fenster, wo `useClipMix` die getrennten Spuren mitzieht.
  useEffect(() => {
    const element = video.current;
    if (element) element.volume = muted ? 0 : volume;
  }, [volume, muted]);

  const toggle = useCallback(() => {
    const element = video.current;
    if (!element) return;
    if (element.paused) void element.play();
    else element.pause();
  }, []);

  const seekTo = useCallback((seconds: number) => {
    const element = video.current;
    if (!element || !Number.isFinite(element.duration)) return;
    jump(element, seconds);
  }, []);

  // Dieselben Tasten wie im Fenster. Escape bleibt beim Elternteil, das backt
  // sich Schritt für Schritt heraus.
  useEffect(() => {
    if (clip.screenshot) return;
    const onKey = (event: KeyboardEvent) => {
      // Im Namensfeld bleiben Leertaste und Buchstaben, was sie sind.
      if (event.target instanceof HTMLInputElement) return;
      const element = video.current;
      if (!element) return;
      if (event.key === " " || event.key.toLowerCase() === "k") toggle();
      else if (event.key === "ArrowRight") jump(element, element.currentTime + 5);
      else if (event.key === "ArrowLeft") jump(element, element.currentTime - 5);
      else if (event.key.toLowerCase() === "m") setMuted((m) => !m);
      else return;
      event.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [clip.screenshot, toggle]);

  const progress = duration > 0 ? (time / duration) * 100 : 0;

  return (
    <div
      onPointerDown={(event) => {
        // Der Player füllt das Fenster, also gehört der Raum um ihn herum ihm:
        // ein Klick neben das Bild heißt „zurück".
        if (event.target === event.currentTarget) onClose();
      }}
      // Nur so viel freihalten, wie das Dock wirklich braucht — es steht bei
      // `inset + 2rem` und ist gut fünf Zeilen hoch. Mit den 12rem von früher
      // wurde die Karte in einem Kasten zentriert, dessen untere Kante viel zu
      // hoch lag, und stand entsprechend zu weit oben.
      className="absolute inset-x-0 top-0 bottom-[calc(var(--inset,0px)+8rem)] grid place-items-center p-6"
    >
      {/* Die Karte nimmt die ganze Höhe und teilt sie auf: das Bild bekommt,
          was Leiste und Namenszeile übrig lassen. Kein `vh` mehr — `.cb-ui`
          trägt `zoom`, und `vh` rechnet gegen das ungezoomte Fenster, sodass
          das Bild bei Größe 1,6 fast den Schirm füllte. */}
      {/* `min()` aus beidem: die Pixelgrenze wächst mit dem Zoom mit, der
          Prozentsatz hält die Karte trotzdem im Bild. */}
      <div className="cb-glass flex h-full w-full max-w-[min(1180px,92%)] flex-col overflow-hidden rounded-card">
        <div className="grid min-h-0 flex-1 place-items-center bg-black/70 p-2">
          {clip.screenshot ? (
            <img
              src={fileUrl(clip.path)}
              alt={clipName(clip)}
              className="h-full w-full rounded-inner object-contain"
            />
          ) : (
            <video
              ref={video}
              src={fileUrl(clip.path)}
              autoPlay
              onClick={toggle}
              onPlay={() => setPlaying(true)}
              // Beim Anhalten übernimmt React die Stelle wieder — sonst spränge
              // sie beim nächsten Rendern dorthin zurück, wo sie vor dem
              // Abspielen stand.
              onPause={(e) => {
                setPlaying(false);
                setTime(e.currentTarget.currentTime);
              }}
              onTimeUpdate={(e) => {
                if (e.currentTarget.paused) setTime(e.currentTarget.currentTime);
              }}
              onSeeked={(e) => setTime(e.currentTarget.currentTime)}
              onLoadedMetadata={(e) => {
                setDuration(e.currentTarget.duration);
                e.currentTarget.volume = muted ? 0 : volume;
              }}
              onEnded={(e) => {
                setPlaying(false);
                setTime(e.currentTarget.currentTime);
              }}
              className="h-full w-full rounded-inner bg-black object-contain"
            />
          )}
        </div>

        {!clip.screenshot && (
          <div className="flex items-center gap-4 border-t border-line px-5 py-3">
            <PlayButton size="sm" playing={playing} onClick={toggle} />
            <Scrubber
              progress={progress}
              fillRef={fill}
              knobRef={knob}
              duration={duration}
              onSeek={(ratio) => seekTo(ratio * duration)}
            />
            <span className="shrink-0 font-mono text-xs text-ink-muted tabular-nums">
              <span ref={clockLabel}>{clock(time)}</span> / {clock(duration)}
            </span>
            <Volume
              value={muted ? 0 : volume}
              onChange={(v) => {
                setVolume(v);
                setMuted(v === 0);
              }}
              onToggleMute={() => setMuted((m) => !m)}
            />
          </div>
        )}

        <div className="flex flex-wrap items-center gap-x-5 gap-y-3 px-5 py-4">
          <div className="min-w-0 flex-1">
            {renaming ? (
              <input
                autoFocus
                defaultValue={clipName(clip)}
                onFocus={(e) => e.currentTarget.select()}
                onKeyDown={(e) => {
                  if (e.key !== "Enter") return;
                  onRename(e.currentTarget.value.trim());
                }}
                onBlur={() => onRenaming(false)}
                className="w-full rounded-inner border border-line bg-base px-2 py-1 text-[15px] font-semibold"
              />
            ) : (
              <button
                onClick={() => onRenaming(true)}
                title="Umbenennen"
                className="group/name flex min-w-0 max-w-full items-center gap-1.5 text-left"
              >
                <span className="truncate text-[15px] font-semibold">{clipName(clip)}</span>
                {/* Der Stift steht nur unter der Maus: dass ein Name ein Feld
                    ist, muss man einmal sehen, nicht dauernd. */}
                <IconPencil className="h-3.5 w-3.5 shrink-0 text-ink-faint opacity-0 transition-opacity group-hover/name:opacity-100" />
              </button>
            )}
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
              onClick={() => onOpenInApp(video.current?.currentTime ?? 0)}
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
      <span className="cb-dock-label relative truncate">
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
        // Entweder oder, nie beides: zwei Hover-Regeln auf derselben
        // Eigenschaft entscheidet sonst die Reihenfolge im Stylesheet.
        danger
          ? "hover:border-live hover:bg-live/15 hover:text-live"
          : "hover:border-accent hover:bg-accent/10 hover:text-accent-bright",
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
    <div className="cb-stat rounded-inner border border-line bg-black/25 p-3">
      <p className="text-[11px] font-bold uppercase tracking-[0.1em] text-ink-muted">{label}</p>
      <p className={cn("mt-1 text-2xl font-bold tabular-nums", bad && "text-live")}>{value}</p>
      <p className="text-xs text-ink-faint">{hint}</p>
    </div>
  );
}
