import { useCallback, useEffect, useRef, useState } from "react";
import { api, fileUrl, inTauri } from "@/lib/ipc";
import { mockTracks } from "@/lib/mock";
import type { Clip, ClipTrack, TrackMix } from "@/lib/types";

export interface TrackState {
  gainDb: number;
  muted: boolean;
}

const NEUTRAL: TrackState = { gainDb: 0, muted: false };

/** dB in linearen Faktor. */
export function gainFactor(db: number): number {
  return 10 ** (db / 20);
}

/**
 * Der nachträgliche Mixer eines Clips.
 *
 * WebView2 spielt von einem MP4 immer nur die erste Tonspur ab — Mikrofon und
 * Discord liegen aber auf eigenen Spuren und wären damit unhörbar. Der Kern
 * entpackt sie deshalb einzeln, und hier laufen sie als eigene Audioelemente
 * am Video mit. Der Export rechnet dieselben Regler später fest ein.
 */
export function useClipMix(
  clip: Clip | undefined,
  video: React.RefObject<HTMLVideoElement | null>,
  master: number,
) {
  const [tracks, setTracks] = useState<ClipTrack[]>([]);
  const [mix, setMix] = useState<Record<number, TrackState>>({});
  const [loading, setLoading] = useState(false);
  const elements = useRef(new Map<number, HTMLAudioElement>());

  const clipId = clip?.id;

  useEffect(() => {
    setTracks([]);
    setMix({});
    if (!clipId) return;

    if (!inTauri) {
      setTracks(mockTracks);
      setMix(Object.fromEntries(mockTracks.map((t) => [t.index, { ...NEUTRAL }])));
      return;
    }

    let cancelled = false;
    setLoading(true);
    api
      .clipTracks(clipId)
      .then((list) => {
        if (cancelled) return;
        setTracks(list);
        setMix(Object.fromEntries(list.map((t) => [t.index, { ...NEUTRAL }])));
      })
      // Ohne Spurenliste bleibt der Player benutzbar, nur der Mixer fehlt.
      .catch(() => undefined)
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [clipId]);

  // Für jede entpackte Spur ein Audioelement. Es hängt nicht im DOM — es soll
  // nur klingen, nicht sichtbar sein.
  useEffect(() => {
    const map = elements.current;
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
  }, [tracks]);

  // Lautstärken. `HTMLMediaElement.volume` kann nur dämpfen, nie anheben —
  // deshalb wird der lauteste Regler zum Bezugspunkt. Die Balance in der
  // Vorschau stimmt damit, nur die Gesamtlautstärke liegt tiefer; im Export
  // wird der Pegel dann wirklich angehoben.
  useEffect(() => {
    const factors = new Map(
      tracks.map((track) => {
        const state = mix[track.index] ?? NEUTRAL;
        return [track.index, state.muted ? 0 : gainFactor(state.gainDb)] as const;
      }),
    );
    const peak = Math.max(1, ...factors.values());
    const volume = (index: number) =>
      Math.min(1, (master * (factors.get(index) ?? 1)) / peak);

    if (video.current) video.current.volume = volume(0);
    for (const [index, audio] of elements.current) audio.volume = volume(index);
  }, [tracks, mix, master, video]);

  // Die Spuren an das Video hängen: starten, anhalten, springen.
  useEffect(() => {
    const element = video.current;
    if (!element) return;
    const all = () => [...elements.current.values()];

    const align = () => {
      for (const audio of all()) {
        if (Math.abs(audio.currentTime - element.currentTime) > 0.12) {
          audio.currentTime = element.currentTime;
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
  }, [tracks, video, clipId]);

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

  /** Die Reglerstellungen so, wie der Export sie erwartet. */
  const toRequest = useCallback(
    (): TrackMix[] =>
      tracks.map((track) => {
        const state = mix[track.index] ?? NEUTRAL;
        return { index: track.index, gainDb: state.gainDb, muted: state.muted };
      }),
    [tracks, mix],
  );

  const touched =
    Object.values(mix).some((state) => state.muted || state.gainDb !== 0);

  return { tracks, mix, loading, setTrack, reset, toRequest, touched };
}
