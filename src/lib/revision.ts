/** Anything the core counts up with each change it takes in — the settings. */
export interface Revised {
  revision: number;
}

/**
 * Whether a config from the core (an answer, an event) may replace the one
 * held: only a later revision. One that left the core earlier is older news —
 * the copy held already has its change, and maybe one made since in another
 * window. A fresh read (`orSame`) also stands in for its own revision: it is
 * what an optimistic copy built on that revision should have been.
 */
export function takes(held: Revised, incoming: Revised, orSame = false): boolean {
  return orSame ? incoming.revision >= held.revision : incoming.revision > held.revision;
}

/** The later of two configs, for one held back while a write is on its way. */
export function later<T extends Revised>(held: T | null, incoming: T): T {
  return held && held.revision >= incoming.revision ? held : incoming;
}
