import { useEffect, type RefObject } from "react";
import { driftAction, slaveTime, steer } from "./slaved";

export interface Slave {
  element: HTMLMediaElement | null;
  /** Element time = master time + lead. */
  leadMs: number;
}

/**
 * Ties muted video elements to the clip's `<video>`: play, pause, seek and rate
 * follow it; drift is pulled back by rate nudges, a large one by a seek. The
 * events are the stems' (`useClipMix`), for the same reasons — `emptied`
 * because a save lets go of the file without a `pause` reaching anyone.
 */
export function useSlavedMedia(
  master: RefObject<HTMLVideoElement | null>,
  slaves: Slave[],
  bindKey: unknown,
) {
  // The slaves' identities and leads, as one dependency.
  const key = slaves.map((s) => `${s.leadMs}`).join(",");
  const elements = slaves.map((s) => s.element);
  useEffect(() => {
    const element = master.current;
    const live = slaves.filter((s): s is { element: HTMLMediaElement; leadMs: number } => !!s.element);
    if (!element || live.length === 0) return;

    const seekAll = () => {
      for (const s of live) {
        s.element.currentTime = slaveTime(element.currentTime, s.leadMs);
        s.element.playbackRate = element.playbackRate;
      }
    };
    const play = () => {
      seekAll();
      for (const s of live) void s.element.play().catch(() => undefined);
    };
    const pause = () => {
      for (const s of live) s.element.pause();
      seekAll();
    };
    const rate = () => {
      for (const s of live) s.element.playbackRate = element.playbackRate;
    };
    // Drift is gathered on every frame the master shows and acted on twice a
    // second, from its average: a single reading of `currentTime` is off by up
    // to a frame either way.
    const sums = live.map(() => ({ total: 0, count: 0, factor: 1 }));
    let lastSteer = performance.now();
    const gather = () => {
      if (element.paused) return;
      live.forEach((s, i) => {
        sums[i].total += s.element.currentTime - slaveTime(element.currentTime, s.leadMs);
        sums[i].count += 1;
      });
      const now = performance.now();
      if (now - lastSteer < 500) return;
      lastSteer = now;
      live.forEach((s, i) => {
        const sum = sums[i];
        if (sum.count === 0) return;
        const drift = sum.total / sum.count;
        sum.total = 0;
        sum.count = 0;
        if (driftAction(drift, "video").seek) {
          s.element.currentTime = slaveTime(element.currentTime, s.leadMs);
          sum.factor = 1;
        } else {
          sum.factor = steer(drift, sum.factor !== 1);
        }
        const rate = element.playbackRate * sum.factor;
        if (s.element.playbackRate !== rate) s.element.playbackRate = rate;
      });
    };

    // Checked on every frame the master shows (WebView2 always offers it).
    let handle = 0;
    let stopped = false;
    const onFrame = () => {
      if (stopped) return;
      gather();
      handle = element.requestVideoFrameCallback(onFrame);
    };
    handle = element.requestVideoFrameCallback(onFrame);

    element.addEventListener("play", play);
    element.addEventListener("playing", play);
    element.addEventListener("pause", pause);
    element.addEventListener("waiting", pause);
    element.addEventListener("ended", pause);
    element.addEventListener("emptied", pause);
    element.addEventListener("seeked", seekAll);
    element.addEventListener("ratechange", rate);
    if (element.paused) seekAll();
    else play();

    return () => {
      stopped = true;
      element.cancelVideoFrameCallback(handle);
      element.removeEventListener("play", play);
      element.removeEventListener("playing", play);
      element.removeEventListener("pause", pause);
      element.removeEventListener("waiting", pause);
      element.removeEventListener("ended", pause);
      element.removeEventListener("emptied", pause);
      element.removeEventListener("seeked", seekAll);
      element.removeEventListener("ratechange", rate);
      for (const s of live) s.element.pause();
    };
    // `slaves` is rebuilt every render; `key` and `elements` are what matter.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [master, key, bindKey, ...elements]);
}
