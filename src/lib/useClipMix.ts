import { useCallback, useEffect, useRef, useState } from "react";
import { api, fileUrl, inTauri } from "@/lib/ipc";
import { mockTracks } from "@/lib/mock";
import type { Clip, ClipTrack, TrackMix } from "@/lib/types";

export interface TrackState {
  gainDb: number;
  muted: boolean;
  /** "Only this track" — hearing just the game audio saves a lot of hunting. */
  solo: boolean;
}

const NEUTRAL: TrackState = { gainDb: 0, muted: false, solo: false };

/**
 * Does the track sound? Solo beats mute: as soon as solo is set anywhere, only
 * the soloed tracks count — exactly how the core handles it while recording.
 */
function audible(state: TrackState, anySolo: boolean): boolean {
  return anySolo ? state.solo : !state.muted;
}

/**
 * Initial slider state. A stored edit brings its mix along; anything not in it
 * starts out neutral.
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

/** dB to a linear factor. */
export function gainFactor(db: number): number {
  return 10 ** (db / 20);
}

/** The sliders, their gain nodes and the way out. */
interface Graph {
  ctx: AudioContext;
  master: GainNode;
  gains: Map<number, GainNode>;
  /** Only for checking whether anything arrives at all — see `SILENT_TICKS`. */
  analyser: AnalyserNode;
}

/** Tick of the silence watchdog. */
const WATCH_MS = 250;

/**
 * This many measurements in a row without a single non-zero sample before the
 * graph counts as dead (2 seconds).
 *
 * A `MediaElementSource` over a foreign origin **without** a CORS grant outputs
 * silence — no error, no warning, and no way to ask beforehand. That would be
 * worse than the bug the graph fixes: the preview would simply be dead. So we
 * listen in and, in doubt, fall back to the old route.
 */
const SILENT_TICKS = 8;

/**
 * Hard clipping to ±1 — the same as what the core does while mixing
 * (`clamp(-1.0, 1.0)`) and ffmpeg does on save. Without it the preview would
 * sound different from the finished clip with the sliders turned up.
 *
 * The curve only needs the straight line from −1 to 1: a WaveShaper maps
 * everything beyond that onto the respective end value by itself.
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
 * A clip's after-the-fact mixer.
 *
 * The clip file has exactly one audio track with everything in it — that is the
 * only way it is heard everywhere. For control, the individual tracks sit beside
 * it; they run along the video here as audio elements of their own, and the
 * video is **muted** for that: its audio track already contains everything and
 * would otherwise play twice.
 *
 * If a clip has no individual tracks (only one audio track, or from before the
 * changeover), the video stays audible and there is nothing to mix.
 */
export function useClipMix(
  clip: Clip | undefined,
  video: React.RefObject<HTMLVideoElement | null>,
  master: number,
  /**
   * Bumped when the video element's source has been set anew. Otherwise the
   * events hang off an element that no longer plays, and the tracks would stop
   * running along.
   */
  bindKey = 0,
) {
  const [tracks, setTracks] = useState<ClipTrack[]>([]);
  const [mix, setMix] = useState<Record<number, TrackState>>({});
  const [loading, setLoading] = useState(false);
  const elements = useRef(new Map<number, HTMLAudioElement>());
  const graph = useRef<Graph | null>(null);
  /** Is the graph up? Only as state does the volume effect notice. */
  const [graphReady, setGraphReady] = useState(false);
  /** Set to "off" as soon as the graph has proven to be silent. */
  const [graphMode, setGraphMode] = useState<"try" | "off">("try");

  const clipId = clip?.id;
  // Are all tracks available as separate files? Only then can anything be
  // mixed — and only then does the video have to stay quiet.
  const separate = tracks.length > 1 && tracks.every((t) => t.previewPath);

  // The stored state, without the track list hanging off it: it is read once on
  // a clip change, after that the sliders belong to the user.
  const saved = useRef(clip?.edit?.tracks);
  saved.current = clip?.edit?.tracks;

  // The individual tracks stay untrimmed and are always in coordinates of the
  // untouched recording. The video file, by contrast, is trimmed and starts at
  // zero. Without this offset the audio of every trimmed clip would come from a
  // different point in the game than the picture.
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
      // Without a track list the player stays usable, only the mixer is missing.
      .catch(() => undefined)
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [clipId]);

  // One audio element per individual track. It does not sit in the DOM — it is
  // supposed to sound, not to be seen. On top of that a small WebAudio graph,
  // because `HTMLMediaElement.volume` can only attenuate and the sliders go up
  // to +12 dB.
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
        // Has to come **before** `src`. The tracks live under
        // `asset.localhost` and therefore on a different origin than the UI;
        // without a CORS grant a MediaElementSource outputs nothing but silence,
        // and does so without any error message.
        audio.crossOrigin = "anonymous";
        audio.src = url;
        audio.preload = "auto";
        map.set(track.index, audio);
      }
    };
    build();

    // If the graph has already proven silent for this clip, it is not built up
    // again at all.
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
        // No WebAudio: then attenuating via `volume` is all that is left. The
        // elements may already be half-attached to the graph and would be silent
        // that way — an element cannot be detached, so create fresh ones.
        built = null;
        drop();
        build();
      }
    }
    graph.current = built;
    setGraphReady(built !== null);

    // Listen in on whether sound really comes out of the graph.
    let watch = 0;
    if (built) {
      const samples = new Float32Array(built.analyser.fftSize);
      let quiet = 0;
      watch = window.setInterval(() => {
        const element = video.current;
        // Only count when sound is expected at all: silence with the video
        // paused or the sliders turned down says nothing.
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

  // Volumes.
  useEffect(() => {
    const anySolo = Object.values(mix).some((state) => state.solo);
    const factors = new Map(
      tracks.map((track) => {
        const state = mix[track.index] ?? NEUTRAL;
        const on = audible(state, anySolo);
        return [track.index, on ? gainFactor(state.gainDb) : 0] as const;
      }),
    );

    // With separated tracks the sound comes exclusively from the audio
    // elements — the video's audio track contains the same thing again.
    if (video.current) video.current.volume = separate ? 0 : master;

    const built = graph.current;
    if (built) {
      // Every slider acts on its own, exactly as it does on save.
      built.master.gain.value = master;
      for (const [index, gain] of built.gains) {
        gain.gain.value = factors.get(index) ?? 1;
      }
      return;
    }

    // Fallback without WebAudio: `HTMLMediaElement.volume` can only attenuate,
    // never boost. So the loudest slider becomes the reference point — the
    // balance is right, the overall level sits lower, and pulling one track above
    // 0 dB makes all the others get quieter. That is exactly why this is only the
    // fallback and no longer the normal case.
    const peak = Math.max(1, ...factors.values());
    for (const [index, audio] of elements.current) {
      audio.volume = Math.min(1, (master * (factors.get(index) ?? 1)) / peak);
    }
    // `bindKey` belongs in here even though it appears nowhere above: after
    // saving, the player mounts a **fresh** video element, and that starts at
    // full volume. `video` is a ref and does not change identity in the process —
    // without this entry the effect would not run again and everything would be
    // heard twice until somebody reopens the clip.
  }, [tracks, mix, master, video, separate, bindKey, graphReady]);

  // Tie the tracks to the video: play, pause, seek.
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
      // A fresh AudioContext is suspended until a user gesture wakes it. Without
      // this the preview would stay silent.
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
    // Saving is not covered by `pause`, however much it looks like it should be.
    // The player pauses and then lets go of the file in the same breath
    // (ClipPlayer.save): `pause()` only *queues* its event, and the `load()` that
    // follows empties the element's task queue before it can be delivered — so
    // the listener above never runs and the tracks played on through the whole
    // write. `emptied` is fired by that same load, after the queue is cleared,
    // and an element with no resource is nothing to play alongside.
    element.addEventListener("emptied", pause);
    element.addEventListener("seeked", align);
    element.addEventListener("ratechange", rate);
    // Two elements run on two clocks; over a minute they drift audibly apart if
    // nobody pulls them back.
    const drift = window.setInterval(() => !element.paused && align(), 1000);
    // The tracks only exist after extraction — the video is long since playing.
    if (!element.paused) play();

    return () => {
      window.clearInterval(drift);
      element.removeEventListener("play", play);
      element.removeEventListener("playing", play);
      element.removeEventListener("pause", pause);
      element.removeEventListener("waiting", pause);
      element.removeEventListener("ended", pause);
      element.removeEventListener("emptied", pause);
      element.removeEventListener("seeked", align);
      element.removeEventListener("ratechange", rate);
      pause();
    };
    // `graphMode` belongs in here: if the preview falls back to the route without
    // WebAudio, the audio elements are freshly created and attached to nothing.
    // Without this entry they would only run again once somebody pauses by hand.
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
   * The slider positions as the core expects them on save. Solo does not exist
   * there — it is resolved into mutes here.
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
