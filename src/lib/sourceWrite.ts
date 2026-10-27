import type { Write } from "./outbox.ts";
import type { AppConfig, AudioSource } from "./types.ts";

/** The fields `next` changes on `shown`. */
export function changedFields(shown: AudioSource, next: AudioSource): Partial<AudioSource> {
  const fields: Record<string, unknown> = {};
  for (const key of Object.keys(next) as (keyof AudioSource)[]) {
    if (JSON.stringify(next[key]) !== JSON.stringify(shown[key])) fields[key] = next[key];
  }
  return fields as Partial<AudioSource>;
}

/**
 * One source row written from a window, for its `Outbox`.
 *
 * The row was built on what the window showed (`shown`; none for a new
 * source). Only the fields changed on it go onto the core's answer when the
 * write's turn comes, and the row goes out with that answer's revision: a
 * level turned in the other window meanwhile stays, and the core merges the
 * row field by field against the copy it names. A row removed elsewhere in
 * the meantime is left removed.
 */
export function sourceWrite(
  next: AudioSource,
  shown: AudioSource | undefined,
  send: (row: AudioSource, revision: number) => Promise<AppConfig>,
  failed?: (err: unknown) => boolean,
): Write<AppConfig> {
  const fields = shown ? changedFields(shown, next) : next;
  return {
    key: `source:${next.id}`,
    change: (config) => {
      const here = config.sources.some((s) => s.id === next.id);
      if (!here && shown) return config;
      const sources = here
        ? config.sources.map((s) => (s.id === next.id ? { ...s, ...fields } : s))
        : [...config.sources, next];
      return { ...config, sources };
    },
    send: async (config, base) => {
      const row = config.sources.find((s) => s.id === next.id);
      return row ? send(row, base.revision) : null;
    },
    failed,
  };
}

/** A source taken off, on the revision of the core's latest answer. */
export function removeSourceWrite(
  id: string,
  send: (id: string, revision: number) => Promise<AppConfig>,
  failed?: (err: unknown) => boolean,
): Write<AppConfig> {
  return {
    change: (config) => ({ ...config, sources: config.sources.filter((s) => s.id !== id) }),
    send: (_config, base) => send(id, base.revision),
    failed,
  };
}
