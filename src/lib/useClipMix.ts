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
  // klingen, nicht sichtbar sein.
  useEffect(() => {
    const map = elements.current;
    if (!separate) return;
    for (const track of tracks) {
      const url = fileUrl(track.previewPath);
      if (!url || map.has(track.index)) continue;
      const audio = new Audio(url);
      audio.preload = "auto";
      map.set(track.index, audio);
    }
    return () => {
      for (const audio of map.values()) {
        audio.pause();
        audio.removeAttribute("src");
        audio.load();
      }
      map.clear();
    };
  }, [tracks, separate]);

  // Lautstärken. `HTMLMediaElement.volume` kann nur dämpfen, nie anheben —
  // deshalb wird der lauteste Regler zum Bezugspunkt. Die Balance in der
  // Vorschau stimmt damit, nur die Gesamtlautstärke liegt tiefer; beim
  // Speichern wird der Pegel dann wirklich angehoben.
  useEffect(() => {
    const anySolo = Object.values(mix).some((state) => state.solo);
    const factors = new Map(
      tracks.map((track) => {
        const state = mix[track.index] ?? NEUTRAL;
        const on = audible(state, anySolo);
        return [track.index, on ? gainFactor(state.gainDb) : 0] as const;
      }),
    );
    const peak = Math.max(1, ...factors.values());
    const volume = (index: number) =>
      Math.min(1, (master * (factors.get(index) ?? 1)) / peak);

    // Bei getrennten Spuren kommt der Ton ausschließlich aus den
    // Audioelementen — die Tonspur des Videos enthält dasselbe noch einmal.
    if (video.current) video.current.volume = separate ? 0 : master;
    for (const [index, audio] of elements.current) audio.volume = volume(index);
    // `bindKey` gehört dazu, obwohl es hier nirgends steht: Nach dem Speichern
    // hängt der Player ein **frisches** Videoelement ein, und das fängt bei
    // voller Lautstärke an. `video` ist ein Ref und ändert seine Identität
    // dabei nicht — ohne diesen Eintrag liefe der Effekt also nicht noch einmal
    // und man hörte alles doppelt, bis jemand den Clip neu öffnet.
  }, [tracks, mix, master, video, separate, bindKey]);

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
  }, [tracks, video, clipId, separate, bindKey, offset]);

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
