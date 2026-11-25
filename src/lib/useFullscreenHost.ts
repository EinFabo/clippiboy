import { useEffect, useState } from "react";

/**
 * Where something floating has to be drawn: into the element in fullscreen
 * while there is one — the player's frame, the only thing shown then, where a
 * card, picker or toast in the body would wait unseen — and into the body
 * otherwise.
 */
export function useFullscreenHost(): Element {
  const [host, setHost] = useState<Element>(() => document.fullscreenElement ?? document.body);
  useEffect(() => {
    const onChange = () => setHost(document.fullscreenElement ?? document.body);
    onChange();
    document.addEventListener("fullscreenchange", onChange);
    return () => document.removeEventListener("fullscreenchange", onChange);
  }, []);
  return host;
}
