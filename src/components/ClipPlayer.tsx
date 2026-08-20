import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/Button";
import { Pill } from "@/components/ui/Card";
import { ClipEditor, type Trim } from "@/components/ClipEditor";
import { IconTrash } from "@/components/icons";
import { useEngine } from "@/store";
import { api, fileUrl, inTauri } from "@/lib/ipc";
import { formatAgo, formatSize } from "@/lib/format";
import { useClipMix } from "@/lib/useClipMix";
import { cn } from "@/lib/cn";
import type { Clip, ClipEdit } from "@/lib/types";

interface Props {
  clips: Clip[];
  index: number;
  onIndexChange: (index: number) => void;
  onClose: () => void;
  onDelete: (id: string) => void;
  /** Zum Audio-Mixer wechseln — dort entstehen die getrennten Tonspuren. */
  onOpenMixer: () => void;
}

/** Ein Einzelbild bei 30 fps. Reicht, um eine Marke sauber zu setzen. */
const FRAME = 1 / 30;

/**
 * Vollflächiger Player über der Galerie, mit dem Bearbeiten-Bereich daneben.
 *
 * Es gibt keinen Bearbeiten-Modus: Name, Tonspuren und Zuschnitt liegen offen,
 * und was eingestellt wird, merkt sich der Kern beim Clip. Die Videodatei
 * Die Bildspur bleibt dabei immer unangetastet — festgerechnet wird nur der
 * Ton, und das erst auf Knopfdruck.
 *
 * Hinweis zu mehreren Tonspuren: WebView2 stellt `HTMLMediaElement.audioTracks`
 * nicht bereit, das Videoelement gibt deshalb immer nur den Hauptmix wieder.
 * Die übrigen Spuren entpackt der Kern einzeln und `useClipMix` lässt sie
 * synchron mitlaufen — nur so lässt sich eine Mischung überhaupt beurteilen.
 */
export function ClipPlayer({
  clips,
  index,
  onIndexChange,
  onClose,
  onDelete,
  onOpenMixer,
}: Props) {
  const clip = clips[index];
  const video = useRef<HTMLVideoElement>(null);
  const frame = useRef<HTMLDivElement>(null);
  const applyClipEdit = useEngine((state) => state.applyClipEdit);

  const [playing, setPlaying] = useState(false);
  const [time, setTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [volume, setVolume] = useState(1);
  const [muted, setMuted] = useState(false);
  const [broken, setBroken] = useState(false);
  const [trim, setTrim] = useState<Trim>({ start: 0, end: 0 });
  const [waveform, setWaveform] = useState<string | undefined>(undefined);
  const [saving, setSaving] = useState(false);
  const [justSaved, setJustSaved] = useState(false);
  // Zählt die Neuanläufe des Videoelements. Dient als `key`: Nach dem
  // Speichern bekommt der Player ein frisches Element statt eines, dem wir
  // die Quelle unter den Füßen weggezogen haben. Die Tonspuren hängen sich
  // daran ebenfalls neu an.
  const [reload, setReload] = useState(0);
  /** Wohin nach dem Neuladen zurückgesprungen wird. */
  const resumeAt = useRef<{ time: number; playing: boolean } | null>(null);
  /** Fehlversuche beim Laden der aktuellen Datei. */
  const loadFailures = useRef(0);

  // Die Lautstärke des Videoelements gehört dem Mixer: Spur 0 ist der
  // Hauptmix, und der muss zu den übrigen Spuren passen.
  const mix = useClipMix(clip, video, muted ? 0 : volume, reload);

  // Die Erfolgsmeldung gehört zu genau einem Clip — beim Blättern wäre sie
  // sonst eine Aussage über einen Clip, den niemand gespeichert hat.
  useEffect(() => {
    setJustSaved(false);
  }, [clip?.id]);

  const step = useCallback(
    (delta: number) => {
      const next = index + delta;
      if (next >= 0 && next < clips.length) onIndexChange(next);
    },
    [clips.length, index, onIndexChange],
  );

  // Beim Clipwechsel alles zurücksetzen.
  useEffect(() => {
    setBroken(false);
    // Sonst spränge der nächste Clip an die Stelle, an der der vorige stand.
    resumeAt.current = null;
    loadFailures.current = 0;
    setTime(0);
    setDuration(0);
    setTrim({ start: 0, end: 0 });
    setWaveform(undefined);
  }, [clip?.id]);

  // Das Bild der Tonspur für die Zeitleiste. Es kommt später als das Video und
  // erscheint dann einfach — fehlt es, bleibt die Leiste wie sie ist.
  const clipId = clip?.id;
  useEffect(() => {
    if (!clipId || !inTauri) return;
    let cancelled = false;
    api
      .clipWaveform(clipId)
      .then((path) => !cancelled && setWaveform(fileUrl(path)))
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [clipId]);

  // Die Position wird beim Abspielen ohne React gezeichnet: Der Frame-Loop
  // schreibt Balken, Griff und Uhr direkt ins DOM.
  //
  // Vorher stand hier ein `setTime` pro Bild, also sechzig Renders in der
  // Sekunde. Der WebView2 rendert React und das Video auf demselben Faden und
  // ließ darüber Videobilder fallen — der Clip sah aus, als ruckelte er und
  // liefe dem Ton davon. Nachgemessen ist die Datei dabei tadellos: 1973 von
  // 1975 Bildabständen exakt 17 ms, Bild und Ton 18 ms auseinander.
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

  // `timeupdate` feuert nur etwa viermal pro Sekunde — die Leiste würde
  // sichtbar springen. Solange abgespielt wird, liest ein Frame-Loop die
  // Position direkt aus dem Element. Der Zuschnitt wird hier mit durchgesetzt.
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    const tick = () => {
      const element = video.current;
      if (element) {
        // Innerhalb der Auswahl bleiben: Am gesetzten Ende zurück an deren
        // Anfang — beim Zuschneiden will man die Stelle mehrfach hören, nicht
        // den Rest des Clips. Lief das Bild von vor der Auswahl herein, wird
        // ebenfalls an den Anfang gezogen.
        if (trim.end > trim.start) {
          const at = element.currentTime;
          if (at >= trim.end || at < trim.start - 0.25) {
            element.currentTime = trim.start;
          }
        }
        paint(element.currentTime, element.duration);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing, trim.start, trim.end, paint]);

  const toggle = useCallback(() => {
    const element = video.current;
    if (!element) return;
    if (!element.paused) {
      element.pause();
      return;
    }
    // Steht die Marke außerhalb der Auswahl, fängt das Abspielen an deren
    // Anfang an — sonst liefe zuerst genau das, was weggeschnitten wurde.
    if (trim.end > trim.start) {
      const at = element.currentTime;
      if (at < trim.start - 0.05 || at >= trim.end - 0.05) {
        element.currentTime = trim.start;
      }
    }
    void element.play();
  }, [trim.start, trim.end]);

  const seek = useCallback((seconds: number) => {
    const element = video.current;
    if (!element || !Number.isFinite(element.duration)) return;
    jump(element, element.currentTime + seconds);
  }, []);

  /** An eine feste Stelle springen und die Anzeige sofort nachziehen. */
  const seekTo = useCallback((seconds: number) => {
    const element = video.current;
    if (!element || !Number.isFinite(element.duration)) return;
    jump(element, seconds);
  }, []);

  const fullscreen = useCallback(() => {
    if (document.fullscreenElement) void document.exitFullscreen();
    else void frame.current?.requestFullscreen();
  }, []);

  /** Anfang oder Ende der Auswahl auf die aktuelle Stelle legen. */
  const mark = useCallback((which: "start" | "end") => {
    const element = video.current;
    if (!element) return;
    const at = element.currentTime;
    setTrim((current) =>
      which === "start"
        ? { start: Math.min(at, current.end - 0.5), end: current.end }
        : { start: current.start, end: Math.max(at, current.start + 0.5) },
    );
  }, []);

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      // In Textfeldern bleiben Leertaste und Buchstaben, was sie sind.
      const target = event.target;
      if (
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement
      ) {
        return;
      }
      const handlers: Record<string, () => void> = {
        " ": toggle,
        k: toggle,
        // Mit Umschalt kleinere Schritte — 5 s springen an der gesuchten
        // Stelle regelmäßig vorbei.
        ArrowRight: () => seek(event.shiftKey ? 1 : 5),
        ArrowLeft: () => seek(event.shiftKey ? -1 : -5),
        // Einzelbilder, um Anfang und Ende genau zu setzen.
        ".": () => seek(FRAME),
        ",": () => seek(-FRAME),
        ArrowUp: () => setVolume((v) => Math.min(1, v + 0.1)),
        ArrowDown: () => setVolume((v) => Math.max(0, v - 0.1)),
        Home: () => seekTo(trim.start),
        End: () => seekTo(Math.max(trim.start, trim.end - FRAME)),
        m: () => setMuted((m) => !m),
        f: fullscreen,
        i: () => mark("start"),
        o: () => mark("end"),
        n: () => step(1),
        p: () => step(-1),
        Escape: () => (document.fullscreenElement ? undefined : onClose()),
      };
      const handler = handlers[event.key] ?? handlers[event.key.toLowerCase()];
      if (!handler) return;
      event.preventDefault();
      handler();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toggle, seek, seekTo, fullscreen, mark, step, onClose, trim.start, trim.end]);

  const base = clip ? fileUrl(clip.path) : undefined;
  // Nach dem Speichern ist die Datei eine andere — meist kürzer, weil der Ton
  // neu gerechnet wurde. Unter derselben Adresse bedient der WebView das
  // Videoelement weiter aus seinem Zwischenspeicher und stellt Bereichsanfragen
  // hinter dem neuen Dateiende. Das meldet das Element als Fehler, und der
  // Player behauptet daraufhin, die Datei sei nicht mehr da. Tauri wertet für
  // den Dateipfad nur `uri().path()` aus, der Anhang stört dort also nicht.
  const source = base && reload > 0 ? `${base}?v=${reload}` : base;
  const current: ClipEdit = {
    startMs: Math.round(trim.start * 1000),
    endMs: Math.round(trim.end * 1000),
    tracks: mix.toRequest(),
  };
  const dirty =
    duration > 0 &&
    mix.tracks.length > 0 &&
    !sameEdit(current, savedEdit(clip?.edit ?? null, duration, current.tracks));

  /**
   * Die Mischung in die Datei schreiben.
   *
   * Der Player muss die Datei dafür loslassen: Windows lässt eine geöffnete
   * Datei nicht ersetzen, und der Kern schreibt den Clip neu. Position und
   * Wiedergabe werden danach wiederhergestellt.
   */
  async function save() {
    if (!clip || saving) return;
    const element = video.current;
    resumeAt.current = {
      time: element?.currentTime ?? 0,
      playing: element ? !element.paused : false,
    };

    setSaving(true);
    setJustSaved(false);
    // Die Datei loslassen: Windows ersetzt keine Datei, die noch offen ist.
    element?.pause();
    element?.removeAttribute("src");
    element?.load();
    try {
      await applyClipEdit(clip.id, current.startMs, current.endMs, current.tracks);
      setJustSaved(true);
    } catch {
      // Der Store hat den Fehler schon als Meldung gesetzt. Hier zählt nur,
      // dass der Player unten wieder ein spielbares Element bekommt.
    } finally {
      setSaving(false);
      // Ein frisches Element statt des abgehängten. Das ist der einzige Weg,
      // der nicht davon abhängt, in welchem Zustand das alte gerade steckt.
      setBroken(false);
      setReload((n) => n + 1);
    }
  }

  // Ein neuer Stand macht die Erfolgsmeldung hinfällig.
  const showSaved = justSaved && !dirty;

  if (!clip) return null;

  const progress = duration > 0 ? (time / duration) * 100 : 0;

  return (
    <div
      className="fixed inset-0 z-50 flex flex-col bg-black/80 backdrop-blur-xl"
      onClick={onClose}
    >
      {/* Der Name des Clips steht im Bearbeiten-Bereich und ist dort
          änderbar — hier oben stünde er ein zweites Mal, aber unantastbar. */}
      <header className="flex shrink-0 items-center justify-between gap-6 px-8 pt-6 pb-4">
        <div className="flex min-w-0 items-center gap-2" onClick={(e) => e.stopPropagation()}>
          <p className="truncate text-xs text-ink-muted">
            {clip.game ?? "Unbekanntes Spiel"} · {formatAgo(clip.createdAt)} ·{" "}
            {formatSize(clip.sizeBytes)}
          </p>
          <Pill className="bg-white/10">{clip.height}p</Pill>
        </div>
        <button
          aria-label="Player schließen"
          onClick={onClose}
          className="grid h-9 w-9 shrink-0 place-items-center rounded-pill border border-line
            text-ink-muted transition-colors hover:bg-elevated hover:text-ink"
        >
          <svg viewBox="0 0 14 14" className="h-3.5 w-3.5" stroke="currentColor" strokeWidth="1.4">
            <path d="M3.5 3.5l7 7M10.5 3.5l-7 7" />
          </svg>
        </button>
      </header>

      <div
        className="flex min-h-0 flex-1 gap-4 px-8"
        onClick={(e) => e.stopPropagation()}
      >
        <div
          ref={frame}
          className="relative min-h-0 min-w-0 flex-1 overflow-hidden rounded-card bg-black"
        >
          {broken || !source ? (
            <div className="grid h-full place-items-center px-8 text-center">
              <div>
                <p className="text-sm font-medium">Datei nicht gefunden</p>
                <p className="mx-auto mt-2 max-w-md text-xs text-ink-muted">
                  {inTauri
                    ? `Die Datei unter ${clip.path} lässt sich nicht öffnen — vermutlich wurde sie außerhalb von ClippiBoy verschoben oder gelöscht.`
                    : "Im Browser-Modus gibt es keine echten Clips."}
                </p>
              </div>
            </div>
          ) : (
            <video
              // Nach dem Speichern ein neues Element — siehe `reload`.
              key={reload}
              ref={video}
              src={source}
              autoPlay
              className="h-full w-full bg-black object-contain"
              onClick={toggle}
              onDoubleClick={fullscreen}
              onPlay={() => setPlaying(true)}
              // Beim Anhalten übernimmt React die Position wieder — sonst
              // spränge sie beim nächsten Render auf den Stand von vor dem
              // Abspielen zurück.
              onPause={(e) => {
                setPlaying(false);
                setTime(e.currentTarget.currentTime);
              }}
              onTimeUpdate={(e) => {
                // Nur noch für den pausierten Zustand und fürs Spulen relevant.
                if (e.currentTarget.paused) setTime(e.currentTarget.currentTime);
              }}
              onSeeked={(e) => setTime(e.currentTarget.currentTime)}
              onLoadedMetadata={(e) => {
                const element = e.currentTarget;
                loadFailures.current = 0;
                const length = element.duration;
                setDuration(length);
                const restored = Number.isFinite(length)
                  ? restoreTrim(clip.edit, length)
                  : null;
                if (restored) setTrim(restored);

                // Nach dem Speichern dort weitermachen, wo man war.
                const resume = resumeAt.current;
                if (resume) {
                  resumeAt.current = null;
                  element.currentTime = resume.time;
                  if (resume.playing) void element.play();
                  else element.pause();
                } else if (restored && restored.start > 0.05) {
                  // Ein zugeschnittener Clip fängt an seiner Marke an, nicht
                  // bei null — sonst sieht man zuerst das Weggeschnittene.
                  element.currentTime = restored.start;
                }
              }}
              onEnded={(e) => {
                setPlaying(false);
                setTime(e.currentTarget.currentTime);
              }}
              // Ein Element ohne Quelle kann nicht an der Datei scheitern —
              // das ist das absichtliche Loslassen beim Speichern. Ein Merker
              // dafür wäre ein Wettlauf gegen dieses Ereignis, das erst später
              // eintrifft; die Quelle selbst zu fragen kann nicht danebengehen.
              onError={(e) => {
                if (!e.currentTarget.getAttribute("src")) return;
                loadFailures.current += 1;
                // Gerade ersetzt worden: Ein einzelner Fehlversuch heißt noch
                // nicht, dass die Datei weg ist. Einmal mit frischer Adresse
                // nachfassen, bevor der Player das behauptet.
                if (loadFailures.current < 2) {
                  setReload((n) => n + 1);
                  return;
                }
                setBroken(true);
              }}
            />
          )}
        </div>

        <ClipEditor
          clip={clip}
          duration={duration}
          tracks={mix.tracks}
          mix={mix.mix}
          mixTouched={mix.touched}
          loadingTracks={mix.loading}
          onTrack={mix.setTrack}
          onResetMix={mix.reset}
          trim={trim}
          onTrim={setTrim}
          onMark={mark}
          separateTracks={mix.separate}
          dirty={dirty}
          saving={saving}
          justSaved={showSaved}
          onSave={save}
          onOpenMixer={onOpenMixer}
        />
      </div>

      <footer
        className="shrink-0 space-y-3 px-8 pt-4 pb-6"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-4">
          <button
            aria-label={playing ? "Pause" : "Abspielen"}
            onClick={toggle}
            disabled={broken}
            className="grid h-11 w-11 shrink-0 place-items-center rounded-pill bg-white text-black
              transition-transform active:scale-95 disabled:opacity-40"
          >
            {playing ? (
              <svg viewBox="0 0 24 24" className="h-4 w-4" fill="currentColor">
                <rect x="6.5" y="5" width="3.6" height="14" rx="1.2" />
                <rect x="13.9" y="5" width="3.6" height="14" rx="1.2" />
              </svg>
            ) : (
              <svg viewBox="0 0 24 24" className="h-4 w-4 translate-x-[1px]" fill="currentColor">
                <path d="M7.5 5.2 19 12 7.5 18.8V5.2Z" />
              </svg>
            )}
          </button>

          <Scrubber
            progress={progress}
            fillRef={fill}
            knobRef={knob}
            duration={duration}
            trim={trim}
            waveform={waveform}
            onSeek={(ratio) => seekTo(ratio * duration)}
            onTrim={setTrim}
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

          <button
            aria-label="Vollbild"
            onClick={fullscreen}
            className="grid h-9 w-9 shrink-0 place-items-center rounded-pill text-ink-muted
              transition-colors hover:bg-elevated hover:text-ink"
          >
            <svg viewBox="0 0 24 24" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.9" strokeLinecap="round" strokeLinejoin="round">
              <path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5" />
            </svg>
          </button>
        </div>

        <div className="flex items-center gap-2">
          <Button
            size="sm"
            variant="secondary"
            onClick={() => inTauri && api.revealClip(clip.id)}
          >
            Im Ordner zeigen
          </Button>
          <Button
            size="sm"
            variant="danger"
            icon={<IconTrash className="h-4 w-4" />}
            onClick={() => {
              onDelete(clip.id);
              if (clips.length <= 1) onClose();
              else onIndexChange(Math.min(index, clips.length - 2));
            }}
          >
            Löschen
          </Button>

          <span className="ml-auto text-xs text-ink-faint">
            Leertaste · ←/→ 5 s, mit Umschalt 1 s · ,/. Einzelbild · I/O Anfang
            &amp; Ende · M stumm · F Vollbild
          </span>

          <div className="flex items-center gap-1">
            <Step label="Vorheriger Clip" disabled={index === 0} onClick={() => step(-1)}>
              ‹
            </Step>
            <span className="w-16 text-center text-xs text-ink-muted tabular-nums">
              {index + 1} / {clips.length}
            </span>
            <Step
              label="Nächster Clip"
              disabled={index >= clips.length - 1}
              onClick={() => step(1)}
            >
              ›
            </Step>
          </div>
        </div>
      </footer>
    </div>
  );
}

/** Springt und hält die Position in den Grenzen der Datei. */
function jump(element: HTMLVideoElement, seconds: number) {
  element.currentTime = Math.min(Math.max(seconds, 0), element.duration);
}

/**
 * Der gespeicherte Zuschnitt, auf die tatsächliche Länge begrenzt. Ohne
 * gespeicherten Stand ist der ganze Clip ausgewählt.
 */
function restoreTrim(edit: ClipEdit | null, duration: number): Trim {
  if (!edit) return { start: 0, end: duration };
  const start = Math.min(Math.max(edit.startMs / 1000, 0), duration);
  const end = Math.min(Math.max(edit.endMs / 1000, start), duration);
  return { start, end: end > start ? end : duration };
}

/**
 * Der zu speichernde Stand — oder `null`, wenn nichts eingestellt ist. Ein
 * unangetasteter Clip soll keinen Eintrag bekommen, sonst gälte jeder
 * angesehene Clip als bearbeitet.
 */
/**
 * Der Stand, der gerade in der Datei steckt. Ein Clip ohne gespeicherten Stand
 * ist der ganze Clip mit unangetasteten Reglern.
 */
function savedEdit(
  edit: ClipEdit | null,
  duration: number,
  tracks: ClipEdit["tracks"],
): ClipEdit {
  return (
    edit ?? {
      startMs: 0,
      endMs: Math.round(duration * 1000),
      tracks: tracks.map((track) => ({ ...track, gainDb: 0, muted: false })),
    }
  );
}

/**
 * Gleicher Stand? Anfang und Ende mit ein paar Millisekunden Spielraum: Die
 * Länge, die das Videoelement meldet, schwankt zwischen zwei Ladevorgängen um
 * Bruchteile — ohne Spielraum gälte jeder frisch geöffnete Clip als geändert.
 */
function sameEdit(a: ClipEdit, b: ClipEdit): boolean {
  const near = (x: number, y: number) => Math.abs(x - y) <= 50;
  return (
    near(a.startMs, b.startMs) &&
    near(a.endMs, b.endMs) &&
    a.tracks.length === b.tracks.length &&
    a.tracks.every((track, i) => {
      const other = b.tracks[i];
      return (
        track.index === other.index &&
        track.gainDb === other.gainDb &&
        track.muted === other.muted
      );
    })
  );
}

function Scrubber({
  progress,
  fillRef,
  knobRef,
  duration,
  trim,
  waveform,
  onSeek,
  onTrim,
}: {
  progress: number;
  /** Balken und Griff. Der Frame-Loop des Players schreibt beim Abspielen
      direkt hinein, statt einen Render auszulösen. */
  fillRef: React.RefObject<HTMLDivElement | null>;
  knobRef: React.RefObject<HTMLDivElement | null>;
  duration: number;
  trim: Trim;
  /** Bild der Tonspur; fehlt, solange ffmpeg es noch zeichnet. */
  waveform?: string;
  onSeek: (ratio: number) => void;
  onTrim: (trim: Trim) => void;
}) {
  const bar = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState<"start" | "end" | null>(null);
  const percent = (seconds: number) =>
    duration > 0 ? Math.min(100, Math.max(0, (seconds / duration) * 100)) : 0;

  /** Einen der beiden Griffe ziehen. */
  const drag = (which: "start" | "end") => (event: React.PointerEvent) => {
    event.preventDefault();
    event.stopPropagation();
    if (duration <= 0) return;
    const box = bar.current?.getBoundingClientRect();
    if (!box) return;
    setDragging(which);

    const move = (moved: PointerEvent) => {
      const ratio = Math.min(
        Math.max((moved.clientX - box.left) / box.width, 0),
        1,
      );
      const at = ratio * duration;
      onTrim(
        which === "start"
          ? { start: Math.min(at, trim.end - 0.5), end: trim.end }
          : { start: trim.start, end: Math.max(at, trim.start + 0.5) },
      );
    };
    const up = () => {
      setDragging(null);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return (
    <div
      ref={bar}
      role="slider"
      aria-label="Position"
      aria-valuenow={Math.round(progress)}
      aria-valuemin={0}
      aria-valuemax={100}
      tabIndex={0}
      onClick={(event) => {
        const box = event.currentTarget.getBoundingClientRect();
        onSeek(Math.min(Math.max((event.clientX - box.left) / box.width, 0), 1));
      }}
      className={cn(
        "group relative h-9 flex-1 cursor-pointer",
        duration === 0 && "pointer-events-none opacity-40",
      )}
    >
      {/* Die Tonspur als Bild: Wo etwas passiert, sieht man vor dem Hinhören. */}
      {waveform && (
        <div
          className="pointer-events-none absolute inset-x-0 inset-y-1 rounded-[3px] opacity-40"
          style={{
            backgroundImage: `url(${waveform})`,
            backgroundSize: "100% 100%",
            backgroundRepeat: "no-repeat",
          }}
        />
      )}

      <div className="absolute inset-x-0 top-1/2 h-1 -translate-y-1/2 rounded-pill bg-white/15">
        {/* Kein width-Übergang: er würde gegen den Frame-Loop arbeiten und
            die Bewegung wieder stockend machen. */}
        <div
          ref={fillRef}
          className="h-full rounded-pill bg-accent-bright"
          style={{ width: `${progress}%` }}
        />
      </div>

      {/* Was wegfällt, liegt hinter einem Schleier. */}
      <div
        className="pointer-events-none absolute inset-y-0 left-0 rounded-l-pill bg-black/55"
        style={{ width: `${percent(trim.start)}%` }}
      />
      <div
        className="pointer-events-none absolute inset-y-0 right-0 rounded-r-pill bg-black/55"
        style={{ width: `${100 - percent(trim.end)}%` }}
      />
      <TrimHandle
        at={percent(trim.start)}
        label="Anfang"
        time={dragging === "start" ? clock(trim.start) : null}
        onDrag={drag("start")}
      />
      <TrimHandle
        at={percent(trim.end)}
        label="Ende"
        time={dragging === "end" ? clock(trim.end) : null}
        onDrag={drag("end")}
      />

      <div
        ref={knobRef}
        className="pointer-events-none absolute top-1/2 h-3 w-3 -translate-x-1/2 -translate-y-1/2
          rounded-pill bg-white opacity-0 transition-opacity group-hover:opacity-100"
        style={{ left: `${progress}%` }}
      />
    </div>
  );
}

function TrimHandle({
  at,
  label,
  time,
  onDrag,
}: {
  at: number;
  label: string;
  /** Beim Ziehen die Stelle anzeigen — sonst schneidet man nach Gefühl. */
  time: string | null;
  onDrag: (event: React.PointerEvent) => void;
}) {
  return (
    <button
      aria-label={`${label} des Ausschnitts`}
      onPointerDown={onDrag}
      onClick={(event) => event.stopPropagation()}
      // Die Trefferfläche ist so hoch wie die Leiste; sichtbar ist nur der
      // Griff in der Mitte. Ein 5 Pixel hohes Ziel trifft niemand zweimal.
      className="group/handle absolute inset-y-0 w-4 -translate-x-1/2 cursor-ew-resize"
      style={{ left: `${at}%` }}
    >
      <span
        className="absolute top-1/2 left-1/2 h-6 w-[3px] -translate-x-1/2 -translate-y-1/2
          rounded-pill bg-accent-bright shadow transition-[height] group-hover/handle:h-7"
      />
      {time && (
        <span
          className="absolute -top-6 left-1/2 -translate-x-1/2 rounded-pill bg-black/80 px-2
            py-0.5 font-mono text-[11px] text-ink tabular-nums"
        >
          {time}
        </span>
      )}
    </button>
  );
}

function Volume({
  value,
  onChange,
  onToggleMute,
}: {
  value: number;
  onChange: (value: number) => void;
  onToggleMute: () => void;
}) {
  return (
    <div className="flex shrink-0 items-center gap-2">
      <button
        aria-label={value === 0 ? "Ton an" : "Stumm"}
        onClick={onToggleMute}
        className="grid h-9 w-9 place-items-center rounded-pill text-ink-muted
          transition-colors hover:bg-elevated hover:text-ink"
      >
        <svg viewBox="0 0 24 24" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinejoin="round">
          <path d="M4 9.5h3.5L12 5.5v13L7.5 14.5H4v-5Z" />
          {value === 0 ? (
            <path d="M16 10l4 4M20 10l-4 4" strokeLinecap="round" />
          ) : (
            <path d="M15.5 9.5a4 4 0 0 1 0 5" strokeLinecap="round" />
          )}
        </svg>
      </button>
      <input
        type="range"
        aria-label="Lautstärke"
        min={0}
        max={1}
        step={0.01}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        className="h-1 w-20 cursor-pointer appearance-none rounded-pill bg-white/15
          [&::-webkit-slider-thumb]:h-3 [&::-webkit-slider-thumb]:w-3
          [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:rounded-pill
          [&::-webkit-slider-thumb]:bg-white"
      />
    </div>
  );
}

function Step({
  label,
  disabled,
  onClick,
  children,
}: {
  label: string;
  disabled: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      aria-label={label}
      disabled={disabled}
      onClick={onClick}
      className="grid h-8 w-8 place-items-center rounded-pill border border-line text-lg
        text-ink-muted transition-colors hover:bg-elevated hover:text-ink
        disabled:pointer-events-none disabled:opacity-30"
    >
      {children}
    </button>
  );
}

/** Wie `formatDuration`, aber für laufende Zeiten (Sekunden statt Millisekunden). */
function clock(seconds: number): string {
  if (!Number.isFinite(seconds)) return "0:00";
  const total = Math.floor(seconds);
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}
