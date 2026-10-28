import { useEffect, useState } from "react";
import { events, inTauri } from "./ipc";

/**
 * Counts the core's `clips-changed` while the component is mounted.
 *
 * The clip folders are granted to the windows on the library thread after
 * the start (`maintain_library`), and a file asked for before that is
 * refused — a player opened in the first seconds showed "File not found"
 * for a clip that is right there. The core says `clips-changed` right after
 * the grant, so a viewer whose file failed tries it again when this goes up.
 * Listening all along rather than from the failure on: an event that came
 * between two failed attempts would otherwise be missed.
 */
export function useGrants(): number {
  const [grants, setGrants] = useState(0);
  useEffect(() => {
    if (!inTauri) return;
    let gone = false;
    let unlisten: (() => void) | undefined;
    void events
      .onClipsChanged(() => setGrants((n) => n + 1))
      .then((stop) => {
        if (gone) stop();
        else unlisten = stop;
      });
    return () => {
      gone = true;
      unlisten?.();
    };
  }, []);
  return grants;
}
