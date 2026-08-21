import { create } from "zustand";
import { api, events, inTauri } from "./lib/ipc";
import * as mock from "./lib/mock";
import type {
  AppConfig,
  AudioDevice,
  AudioProcess,
  AudioSource,
  CaptureTarget,
  Clip,
  EncoderInfo,
  LevelMap,
  TrackMix,
} from "./lib/types";

interface EngineState {
  ready: boolean;
  config: AppConfig;
  devices: AudioDevice[];
  processes: AudioProcess[];
  targets: CaptureTarget[];
  encoders: EncoderInfo[];
  clips: Clip[];
  levels: LevelMap;
  sourceErrors: Record<string, string>;
  /** The source runs, but doubled or on a fallback clock — not an error, but
   *  something you want to know before recording. */
  sourceWarnings: Record<string, string>;
  bufferActive: boolean;
  bufferedSeconds: number;
  /** Game detected in the foreground, reported by the core. */
  detectedGame: string | null;
  lastError: string | null;

  init: () => Promise<void>;
  refreshSources: () => Promise<void>;
  refreshTargets: () => Promise<void>;
  patchConfig: (patch: Partial<AppConfig>) => Promise<void>;
  /** Both hotkeys at once — the core only accepts them together. */
  setHotkeys: (saveClip: string, toggleBuffer: string) => Promise<void>;
  setClipDir: (dir: string) => Promise<void>;
  upsertSource: (source: AudioSource) => Promise<void>;
  removeSource: (id: string) => Promise<void>;
  toggleBuffer: () => Promise<void>;
  saveClip: () => Promise<void>;
  deleteClip: (id: string) => Promise<void>;
  updateClip: (id: string, meta: ClipMeta) => Promise<void>;
  /** Set or take away the heart. */
  setFavorite: (id: string, favorite: boolean) => Promise<void>;
  /**
   * Move the file into its folder — after a change of game or heart. Kept apart
   * from editing so the player does not lose its file mid-playback: it only calls
   * this on close.
   */
  fileClip: (id: string) => Promise<void>;
  /** Remove one game from every clip — the clips themselves stay. */
  clearGame: (game: string) => Promise<void>;
  /**
   * Den Clip so schreiben, wie er im Editor steht: Mischung eingerechnet,
   * trim carried out. Touches the file itself, not just the database.
   */
  applyClipEdit: (
    id: string,
    startMs: number,
    endMs: number,
    tracks: TrackMix[],
  ) => Promise<void>;
  /** Undo the trim and pull the whole recording back. */
  restoreClipOriginal: (id: string) => Promise<void>;
}

/** A clip's hand-editable fields. */
export interface ClipMeta {
  title: string | null;
  description: string | null;
  game: string | null;
}

export const useEngine = create<EngineState>((set, get) => ({
  ready: false,
  config: mock.mockConfig,
  devices: [],
  processes: [],
  targets: [],
  encoders: [],
  clips: [],
  levels: {},
  sourceErrors: {},
  sourceWarnings: {},
  bufferActive: false,
  bufferedSeconds: 0,
  detectedGame: null,
  lastError: null,

  async init() {
    if (!inTauri) {
      // Browser mode: load the mocks, simulate levels.
      set({
        ready: true,
        config: mock.mockConfig,
        devices: mock.mockDevices,
        processes: mock.mockProcesses,
        targets: mock.mockTargets,
        encoders: mock.mockEncoders,
        clips: mock.mockClips,
      });
      setInterval(() => {
        const levels: LevelMap = {};
        for (const s of get().config.sources) {
          levels[s.id] =
            s.enabled && !s.muted ? Math.random() * 0.55 + 0.15 : 0;
        }
        set({ levels });
      }, 80);
      return;
    }

    // Every query on its own: if one fails — the process list, say, for which
    // Windows does not always grant rights — that must not take the rest down
    // with it. Previously the target list stayed empty in that case too and no
    // source could be picked on the recording page.
    const fail = (what: string, err: unknown) => {
      console.error(`${what} fehlgeschlagen`, err);
      set({ lastError: `${what}: ${String(err)}` });
    };
    const load = async <T>(
      what: string,
      call: () => Promise<T>,
      fallback: T,
    ): Promise<T> => {
      try {
        return await call();
      } catch (err) {
        fail(what, err);
        return fallback;
      }
    };

    try {
      const [config, devices, processes, targets, encoders, clips] =
        await Promise.all([
          load("Konfiguration lesen", api.getConfig, get().config),
          load("read audio devices", api.listAudioDevices, []),
          load("Anwendungen lesen", api.listAudioProcesses, []),
          load("Aufnahmequellen lesen", api.listCaptureTargets, []),
          load("Encoder lesen", api.listEncoders, []),
          load("Clips lesen", api.listClips, []),
        ]);
      set({ ready: true, config, devices, processes, targets, encoders, clips });

      await events.onLevels((levels) => set({ levels }));
      await events.onStatus((s) =>
        set({
          bufferActive: s.bufferActive,
          bufferedSeconds: s.bufferedSeconds,
          detectedGame: s.game,
        }),
      );
      // Hotkey, tray and button all run through the same path in the core and
      // report back here — which is why the clip is only added at this one spot.
      await events.onClipSaved((clip) =>
        set((st) => ({
          clips: [clip, ...st.clips.filter((c) => c.id !== clip.id)],
        })),
      );
      await events.onAudioErrors((sourceErrors) => set({ sourceErrors }));
      await events.onAudioWarnings((sourceWarnings) => set({ sourceWarnings }));
    } catch (err) {
      set({ ready: true, lastError: String(err) });
    }
  },

  async refreshSources() {
    if (!inTauri) return;
    const [devices, processes, targets] = await Promise.all([
      api.listAudioDevices().catch(() => get().devices),
      api.listAudioProcesses().catch(() => get().processes),
      api.listCaptureTargets().catch(() => get().targets),
    ]);
    set({ devices, processes, targets });
  },

  /** Re-read only the video sources — monitors and open windows. */
  async refreshTargets() {
    if (!inTauri) return;
    try {
      set({ targets: await api.listCaptureTargets() });
    } catch (err) {
      set({ lastError: `Aufnahmequellen lesen: ${String(err)}` });
    }
  },

  async patchConfig(patch) {
    const config = { ...get().config, ...patch };
    set({ config });
    if (!inTauri) return;
    try {
      await api.setConfig(config);
    } catch (err) {
      // The selection already stands in the UI — if the core does not accept it,
      // somebody has to see that rather than have it fail silently.
      set({ lastError: `Apply setting: ${String(err)}` });
    }
  },

  async setHotkeys(saveClip, toggleBuffer) {
    if (!inTauri) {
      set((st) => ({
        config: {
          ...st.config,
          saveClipHotkey: saveClip,
          toggleBufferHotkey: toggleBuffer,
        },
      }));
      return;
    }
    // Deliberately without an optimistic update: if registering fails, the old
    // assignment should stand, and the error belongs on that row.
    set({ config: await api.setHotkeys(saveClip, toggleBuffer) });
  },

  async setClipDir(dir) {
    if (!inTauri) {
      set((st) => ({ config: { ...st.config, clipDir: dir } }));
      return;
    }
    set({ config: await api.setClipDir(dir) });
  },

  async upsertSource(source) {
    const sources = get().config.sources;
    const exists = sources.some((s) => s.id === source.id);
    const next = exists
      ? sources.map((s) => (s.id === source.id ? source : s))
      : [...sources, source];
    set({ config: { ...get().config, sources: next } });
    if (inTauri) {
      const config = exists
        ? await api.updateAudioSource(source)
        : await api.addAudioSource(source);
      set({ config });
    }
  },

  async removeSource(id) {
    const next = get().config.sources.filter((s) => s.id !== id);
    set({ config: { ...get().config, sources: next } });
    if (inTauri) set({ config: await api.removeAudioSource(id) });
  },

  async toggleBuffer() {
    const active = get().bufferActive;
    set({ bufferActive: !active });
    if (inTauri) {
      try {
        active ? await api.stopBuffer() : await api.startBuffer();
      } catch (err) {
        set({ bufferActive: active, lastError: String(err) });
      }
    }
  },

  async saveClip() {
    if (!inTauri) return;
    try {
      // The core reports the finished clip back via `clip-saved`.
      await api.saveClip();
    } catch (err) {
      set({ lastError: String(err) });
    }
  },

  async deleteClip(id) {
    set((st) => ({ clips: st.clips.filter((c) => c.id !== id) }));
    if (inTauri) await api.deleteClip(id);
  },

  async updateClip(id, meta) {
    // Apply locally first: typing happens in a field that should show every
    // keystroke right away, saving runs alongside.
    set((st) => ({
      clips: st.clips.map((c) => (c.id === id ? { ...c, ...meta } : c)),
    }));
    if (!inTauri) return;
    try {
      const clip = await api.updateClip(id, meta);
      set((st) => ({ clips: st.clips.map((c) => (c.id === id ? clip : c)) }));
    } catch (err) {
      set({ lastError: String(err) });
    }
  },

  async setFavorite(id, favorite) {
    // The heart has to flip with no delay — a click you only see once the
    // database has answered feels broken.
    set((st) => ({
      clips: st.clips.map((c) => (c.id === id ? { ...c, favorite } : c)),
    }));
    if (!inTauri) return;
    try {
      const clip = await api.setClipFavorite(id, favorite);
      set((st) => ({ clips: st.clips.map((c) => (c.id === id ? clip : c)) }));
    } catch (err) {
      set((st) => ({
        clips: st.clips.map((c) =>
          c.id === id ? { ...c, favorite: !favorite } : c,
        ),
        lastError: String(err),
      }));
    }
  },

  async fileClip(id) {
    if (!inTauri) return;
    try {
      const clip = await api.fileClip(id);
      set((st) => ({ clips: st.clips.map((c) => (c.id === id ? clip : c)) }));
    } catch (err) {
      // The folder is cosmetic; the clip itself is right either way.
      console.error("Clip einsortieren fehlgeschlagen", err);
    }
  },

  /**
   * Clears away a filter that is not one: misdetections like a browser window
   * would otherwise stand in the gallery forever. The clips stay untouched and
   * are called "Unknown" afterwards.
   */
  async clearGame(game) {
    const affected = get().clips.filter((c) => c.game === game);
    if (affected.length === 0) return;

    set((st) => ({
      clips: st.clips.map((c) => (c.game === game ? { ...c, game: null } : c)),
    }));
    if (!inTauri) return;
    try {
      // The core only knows individual clips; one after another, so the SQLite
      // connection does not have to fight parallel writers.
      for (const clip of affected) {
        await api.updateClip(clip.id, {
          title: clip.title,
          description: clip.description,
          game: null,
        });
        // With no game the file belongs back in the clip folder.
        await get().fileClip(clip.id);
      }
    } catch (err) {
      set({ lastError: String(err) });
    }
  },

  async applyClipEdit(id, startMs, endMs, tracks) {
    if (!inTauri) return;
    // Deliberately without an optimistic update: a file is rewritten here. If
    // that fails — because it is still open, say — the UI must not
    // behaupten, es sei gespeichert.
    try {
      const clip = await api.applyClipEdit(id, startMs, endMs, tracks);
      set((st) => ({ clips: st.clips.map((c) => (c.id === id ? clip : c)) }));
    } catch (err) {
      set({ lastError: String(err) });
      throw err;
    }
  },

  async restoreClipOriginal(id) {
    if (!inTauri) return;
    try {
      const clip = await api.restoreClipOriginal(id);
      set((st) => ({ clips: st.clips.map((c) => (c.id === id ? clip : c)) }));
    } catch (err) {
      set({ lastError: String(err) });
      throw err;
    }
  },
}));
