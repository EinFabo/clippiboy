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
  bufferActive: boolean;
  bufferedSeconds: number;
  /** Im Vordergrund erkanntes Spiel, vom Kern gemeldet. */
  detectedGame: string | null;
  lastError: string | null;

  init: () => Promise<void>;
  refreshSources: () => Promise<void>;
  patchConfig: (patch: Partial<AppConfig>) => Promise<void>;
  /** Beide Hotkeys auf einmal — der Kern nimmt sie nur zusammen an. */
  setHotkeys: (saveClip: string, toggleBuffer: string) => Promise<void>;
  setClipDir: (dir: string) => Promise<void>;
  upsertSource: (source: AudioSource) => Promise<void>;
  removeSource: (id: string) => Promise<void>;
  toggleBuffer: () => Promise<void>;
  saveClip: () => Promise<void>;
  deleteClip: (id: string) => Promise<void>;
  updateClip: (id: string, meta: ClipMeta) => Promise<void>;
}

/** Die von Hand pflegbaren Felder eines Clips. */
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
  bufferActive: false,
  bufferedSeconds: 0,
  detectedGame: null,
  lastError: null,

  async init() {
    if (!inTauri) {
      // Browser-Modus: Mocks laden, Pegel simulieren.
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

    try {
      const [config, devices, processes, targets, encoders, clips] =
        await Promise.all([
          api.getConfig(),
          api.listAudioDevices(),
          api.listAudioProcesses(),
          api.listCaptureTargets(),
          api.listEncoders(),
          api.listClips(),
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
      // Hotkey, Tray und Button laufen im Kern über denselben Pfad und melden
      // sich alle hierüber — deshalb wird der Clip nur an dieser Stelle ergänzt.
      await events.onClipSaved((clip) =>
        set((st) => ({
          clips: [clip, ...st.clips.filter((c) => c.id !== clip.id)],
        })),
      );
      await events.onAudioErrors((sourceErrors) => set({ sourceErrors }));
    } catch (err) {
      set({ ready: true, lastError: String(err) });
    }
  },

  async refreshSources() {
    if (!inTauri) return;
    const [devices, processes, targets] = await Promise.all([
      api.listAudioDevices(),
      api.listAudioProcesses(),
      api.listCaptureTargets(),
    ]);
    set({ devices, processes, targets });
  },

  async patchConfig(patch) {
    const config = { ...get().config, ...patch };
    set({ config });
    if (inTauri) await api.setConfig(config);
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
    // Bewusst ohne Vorgriff im Store: schlägt das Registrieren fehl, soll die
    // alte Belegung stehen bleiben, und die Fehlermeldung gehört an die Zeile.
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
      // Der Kern meldet den fertigen Clip über `clip-saved` zurück.
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
    // Erst lokal übernehmen: Getippt wird in ein Feld, das jeden Anschlag
    // sofort zeigen soll, gespeichert wird nebenher.
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
}));
