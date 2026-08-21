import { useCallback, useEffect, useRef, useState } from "react";
import { api, fileUrl, inTauri } from "@/lib/ipc";
import { mockTracks } from "@/lib/mock";
import type { Clip, ClipTrack, TrackMix } from "@/lib/types";

export interface TrackState {
  gainDb: number;
  muted: boolean;
  /** „Nur diese Spur" — hört man nur den Spielton, sucht man nicht lange. */
  solo: boolean;
}

const NEUTRAL: TrackState = { gainDb: 0, muted: false, solo: false };

/**
 * Klingt die Spur? Solo schlägt Stumm: Sobald irgendwo Solo steht, zählen nur
 * noch die Solospuren — genauso hält es der Kern beim Aufnehmen.
 */
function audible(state: TrackState, anySolo: boolean): boolean {
  return anySolo ? state.solo : !state.muted;
}

/**
 * Startzustand der Regler. Ein gespeicherter Schnitt bringt seine Mischung
 * mit; alles, was dort nicht steht, fängt neutral an.
 */
function initialMix(
  list: ClipTrack[],
  saved: TrackMix[] | undefined,
): Record<number, TrackState> {
  const stored = new Map(saved?.map((track) => [track.index, track]));
  return Object.fromEntries(
    list.map((track) => {
      const state = stored.get(track.index);
      return [
        track.index,
        state
          ? { gainDb: state.gainDb, muted: state.muted, solo: false }
          : { ...NEUTRAL },
      ];
    }),
  );
}

/** dB in linearen Faktor. */
export function gainFactor(db: number): number {
  return 10 ** (db / 20);
}

/** Die Regler, ihre Verstärker und der Weg nach draußen. */
interface Graph {
  ctx: AudioContext;
  master: GainNode;
  gains: Map<number, GainNode>;
  /** Nur zum Nachhören, ob überhaupt etwas ankommt — siehe `SILENT_TICKS`. */
  analyser: AnalyserNode;
}

/** Takt der Stummheitswache. */
const WATCH_MS = 250;

/**
 * So viele Messungen in Folge ohne ein einziges Sample ungleich null, bis der
 * Graph als tot gilt (2 Sekunden).
 *
 * Ein `MediaElementSource` über einer fremden Herkunft **ohne** CORS-Freigabe
 * gibt Stille aus — ohne Fehler, ohne Warnung und ohne dass es sich vorher
 * abfragen ließe. Das wäre schlimmer als der Fehler, den der Graph behebt: Die
 * Vorschau wäre schlicht tot. Deshalb wird nachgehört und im Zweifel auf den
 * alten Weg zurückgefallen.
 */
const SILENT_TICKS = 8;

/**
 * Hartes Begrenzen auf ±1 — dasselbe, was der Kern beim Mischen
 * (`clamp(-1.0, 1.0)`) und ffmpeg beim Speichern tun. Ohne das klänge die
 * Vorschau bei aufgedrehten Reglern anders als der fertige Clip.
 *
 * Der Kennlinie reicht die Gerade von −1 bis 1: Alles darüber hinaus bildet ein
 * WaveShaper von sich aus auf den jeweiligen Endwert ab.
 */
function hardClip(ctx: AudioContext): WaveShaperNode {
  const shaper = ctx.createWaveShaper();
  const points = 1024;
  const curve = new Float32Array(points);
  for (let i = 0; i < points; i += 1) {
    curve[i] = (i / (points - 1)) * 2 - 1;
  }
  shaper.curve = curve;
  return shaper;
}

/**
 * Der nachträgliche Mixer eines Clips.
 *
 * Die Clipdatei hat genau eine Tonspur, in der alles steckt — nur so hört man
 * sie überall. Zum Regeln liegen die Einzelspuren daneben; die laufen hier als
 * eigene Audioelemente am Video mit, und das Video wird dafür **stumm**
 * geschaltet: Seine Tonspur enthält ja bereits alles und liefe sonst doppelt.
 *
 * Hat ein Clip keine Einzelspuren (nur eine Tonspur, oder von vor der
 * Umstellung), bleibt das Video hörbar und es gibt nichts zu mischen.
 */
export function useClipMix(
  clip: Clip | undefined,
  video: React.RefObject<HTMLVideoElement | null>,
  master: number,
  /**
   * Hochzählen, wenn die Quelle des Videoelements neu gesetzt wurde. Die
   * Ereignisse hängen sonst an einem Element, das nicht mehr abspielt, und die
   * Spuren liefen nicht mehr mit.
   */
  bindKey = 0,
) {
  const [tracks, setTracks] = useState<ClipTrack[]>([]);
  const [mix, setMix] = useState<Record<number, TrackState>>({});
  const [loading, setLoading] = useState(false);
  const elements = useRef(new Map<number, HTMLAudioElement>());
  const graph = useRef<Graph | null>(null);
  /** Steht der Graph? Nur als Zustand merkt es der Lautstärke-Effekt. */
  const [graphReady, setGraphReady] = useState(false);
  /** Auf „off" gestellt, sobald sich der Graph als stumm erwiesen hat. */
  const [graphMode, setGraphMode] = useState<"try" | "off">("try");

  const clipId = clip?.id;
  // Liegen alle Spuren als eigene Dateien vor? Nur dann lässt sich mischen —
  // und nur dann muss das Video schweigen.
  const separate = tracks.length > 1 && tracks.every((t) => t.previewPath);

  // Der gespeicherte Stand, ohne dass die Spurenliste an ihm hängt: Er wird
  // beim Clipwechsel einmal gelesen, danach gehören die Regler dem Nutzer.
  const saved = useRef(clip?.edit?.tracks);
  saved.current = clip?.edit?.tracks;

  // Die Einzelspuren bleiben ungeschnitten und stehen immer in Koordinaten der
  // unversehrten Aufnahme. Die Videodatei ist dagegen geschnitten und fängt bei
  // null an. Ohne diesen Versatz käme der Ton bei jedem geschnittenen Clip aus
  // einer anderen Stelle des Spiels als das Bild.
  const offset = (clip?.original?.startMs ?? 0) / 1000;

  useEffect(() => {
    setTracks([]);
    setMix({});
    setGraphMode("try");
    if (!clipId) return;

    if (!inTauri) {
      setTracks(mockTracks);
      setMix(initialMix(mockTracks, saved.current));
      return;
    }

    let cancelled = false;
    setLoading(true);
    api
      .clipTracks(clipId)
      .then((list) => {
        if (cancelled) return;
        setTracks(list);
        setMix(initialMix(list, saved.current));
      })
      // Ohne Spurenliste bleibt der Player benutzbar, nur der Mixer fehlt.
      .catch(() => undefined)
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [clipId]);

  // Für jede Einzelspur ein Audioelement. Es hängt nicht im DOM — es soll nur
  // klingen, nicht sichtbar sein. Darüber ein kleiner WebAudio-Graph, weil
  // `HTMLMediaElement.volume` nur dämpfen kann und die Regler bis +12 dB gehen.
  useEffect(() => {
    const map = elements.current;
    if (!separate) return;

    const drop = () => {
      for (const audio of map.values()) {
        audio.pause();
        audio.removeAttribute("src");
        audio.load();
      }
      map.clear();
    };
    const build = () => {
      for (const track of tracks) {
        const url = fileUrl(track.previewPath);
        if (!url || map.has(track.index)) continue;
        const audio = new Audio();
        // Muss **vor** `src` stehen. Die Spuren liegen unter
        // `asset.localhost` und damit auf einer anderen Herkunft als die
        // Oberfläche; ohne CORS-Freigabe gibt ein MediaElementSource nur
        // Stille aus, und zwar ohne jede Fehlermeldung.
        audio.crossOrigin = "anonymous";
        audio.src = url;
        audio.preload = "auto";
        map.set(track.index, audio);
      }
    };
    build();

    // Hat sich der Graph für diesen Clip schon als stumm erwiesen, wird er gar
    // nicht erst wieder aufgebaut.
    let built: Graph | null = null;
    if (graphMode === "try") {
      try {
        const ctx = new AudioContext();
        const master = ctx.createGain();
        const analyser = ctx.createAnalyser();
        master.connect(analyser);
        analyser.connect(hardClip(ctx)).connect(ctx.destination);

        const gains = new Map<number, GainNode>();
        for (const [index, audio] of map) {
          const gain = ctx.createGain();
          ctx.createMediaElementSource(audio).connect(gain).connect(master);
          gains.set(index, gain);
        }
        built = { ctx, master, gains, analyser };
      } catch {
        // Kein WebAudio: Dann bleibt es beim Dämpfen über `volume`. Die
        // Elemente hängen womöglich schon halb im Graphen und wären damit
        // stumm — herauslösen lässt sich ein Element nicht, also neu anlegen.
        built = null;
        drop();
        build();
      }
    }
    graph.current = built;
    setGraphReady(built !== null);

    // Nachhören, ob aus dem Graphen wirklich Ton kommt.
    let watch = 0;
    if (built) {
      const samples = new Float32Array(built.analyser.fftSize);
      let quiet = 0;
      watch = window.setInterval(() => {
        const element = video.current;
        // Nur zählen, wenn überhaupt Ton zu erwarten ist: Stille bei
        // angehaltenem Video oder zugedrehten Reglern sagt nichts.
        if (!element || element.paused || built.ctx.state !== "running") return;
        if (built.master.gain.value <= 0) return;
        if ([...built.gains.values()].every((gain) => gain.gain.value <= 0)) return;

        built.analyser.getFloatTimeDomainData(samples);
        quiet = samples.some((value) => value !== 0) ? 0 : quiet + 1;
        if (quiet >= SILENT_TICKS) {
          window.clearInterval(watch);
          setGraphMode("off");
        }
      }, WATCH_MS);
    }

    return () => {
      window.clearInterval(watch);
      graph.current = null;
      setGraphReady(false);
      if (built) {
        built.master.disconnect();
        void built.ctx.close().catch(() => undefined);
      }
      drop();
    };
  }, [tracks, separate, graphMode, video]);

  // Lautstärken.
  useEffect(() => {
    const anySolo = Object.values(mix).some((state) => state.solo);
    const factors = new Map(
      tracks.map((track) => {
        const state = mix[track.index] ?? NEUTRAL;
        const on = audible(state, anySolo);
        return [track.index, on ? gainFactor(state.gainDb) : 0] as const;
      }),
    );

    // Bei getrennten Spuren kommt der Ton ausschließlich aus den
    // Audioelementen — die Tonspur des Videos enthält dasselbe noch einmal.
    if (video.current) video.current.volume = separate ? 0 : master;

    const built = graph.current;
    if (built) {
      // Jeder Regler wirkt für sich, genau wie beim Speichern.
      built.master.gain.value = master;
      for (const [index, gain] of built.gains) {
        gain.gain.value = factors.get(index) ?? 1;
      }
      return;
    }

    // Notweg ohne WebAudio: `HTMLMediaElement.volume` kann nur dämpfen, nie
    // anheben. Der lauteste Regler wird deshalb zum Bezugspunkt — die Balance
    // stimmt, die Gesamtlautstärke liegt tiefer, und wer eine Spur über 0 dB
    // zieht, hört alle anderen leiser werden. Genau deshalb ist das nur der
    // Notweg und nicht mehr der Normalfall.
    const peak = Math.max(1, ...factors.values());
    for (const [index, audio] of elements.current) {
      audio.volume = Math.min(1, (master * (factors.get(index) ?? 1)) / peak);
    }
    // `bindKey` gehört dazu, obwohl es hier nirgends steht: Nach dem Speichern
    // hängt der Player ein **frisches** Videoelement ein, und das fängt bei
    // voller Lautstärke an. `video` ist ein Ref und ändert seine Identität
    // dabei nicht — ohne diesen Eintrag liefe der Effekt also nicht noch einmal
    // und man hörte alles doppelt, bis jemand den Clip neu öffnet.
  }, [tracks, mix, master, video, separate, bindKey, graphReady]);

  // Die Spuren an das Video hängen: starten, anhalten, springen.
  useEffect(() => {
    const element = video.current;
    if (!element || !separate) return;
    const all = () => [...elements.current.values()];

    const align = () => {
      const at = element.currentTime + offset;
      for (const audio of all()) {
        if (Math.abs(audio.currentTime - at) > 0.12) {
          audio.currentTime = at;
        }
      }
    };
    const play = () => {
      // Ein frischer AudioContext ist angehalten, bis ihn eine Nutzeraktion
      // weckt. Ohne das bliebe die Vorschau stumm.
      void graph.current?.ctx.resume().catch(() => undefined);
      align();
      for (const audio of all()) void audio.play().catch(() => undefined);
    };
    const pause = () => {
      for (const audio of all()) audio.pause();
    };
    const rate = () => {
      for (const audio of all()) audio.playbackRate = element.playbackRate;
    };

    element.addEventListener("play", play);
    element.addEventListener("playing", play);
    element.addEventListener("pause", pause);
    element.addEventListener("waiting", pause);
    element.addEventListener("ended", pause);
    element.addEventListener("seeked", align);
    element.addEventListener("ratechange", rate);
    // Zwei Elemente laufen auf zwei Uhren; über eine Minute driften sie
    // hörbar auseinander, wenn niemand nachzieht.
    const drift = window.setInterval(() => !element.paused && align(), 1000);
    // Die Spuren sind erst nach dem Entpacken da — das Video läuft dann längst.
    if (!element.paused) play();

    return () => {
      window.clearInterval(drift);
      element.removeEventListener("play", play);
      element.removeEventListener("playing", play);
      element.removeEventListener("pause", pause);
      element.removeEventListener("waiting", pause);
      element.removeEventListener("ended", pause);
      element.removeEventListener("seeked", align);
      element.removeEventListener("ratechange", rate);
      pause();
    };
    // `graphMode` gehört dazu: Fällt die Vorschau auf den Weg ohne WebAudio
    // zurück, sind die Audioelemente frisch angelegt und hängen an nichts mehr.
    // Ohne diesen Eintrag liefen sie erst wieder, wenn jemand von Hand anhält.
  }, [tracks, video, clipId, separate, bindKey, offset, graphMode]);

  const setTrack = useCallback((index: number, patch: Partial<TrackState>) => {
    setMix((current) => ({
      ...current,
      [index]: { ...(current[index] ?? NEUTRAL), ...patch },
    }));
  }, []);

  const reset = useCallback(() => {
    setMix((current) =>
      Object.fromEntries(Object.keys(current).map((key) => [key, { ...NEUTRAL }])),
    );
  }, []);

  /**
   * Die Reglerstellungen so, wie der Kern sie beim Speichern erwartet. Solo
   * gibt es dort nicht — es wird hier in Stummschaltungen aufgelöst.
   */
  const toRequest = useCallback((): TrackMix[] => {
    const anySolo = Object.values(mix).some((state) => state.solo);
    return tracks.map((track) => {
      const state = mix[track.index] ?? NEUTRAL;
      return {
        index: track.index,
        gainDb: state.gainDb,
        muted: !audible(state, anySolo),
      };
    });
  }, [tracks, mix]);

  const touched = Object.values(mix).some(
    (state) => state.muted || state.solo || state.gainDb !== 0,
  );

  return { tracks, mix, loading, setTrack, reset, toRequest, touched, separate };
}
