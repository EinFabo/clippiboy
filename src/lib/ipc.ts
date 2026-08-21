import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppConfig,
  AudioDevice,
  AudioProcess,
  AudioSource,
  CaptureTarget,
  Clip,
  ClipProgress,
  ClipTrack,
  EncoderInfo,
  EngineStatus,
  LevelMap,
  TrackMix,
  UpdateInfo,
} from "./types";

/** Läuft die App im Tauri-Container? (Im reinen Browser-Vite fehlt das IPC.) */
export const inTauri = "__TAURI_INTERNALS__" in window;

/// Lokale Datei in eine für den WebView ladbare URL übersetzen.
export function fileUrl(path: string | null): string | undefined {
  if (!path || !inTauri) return undefined;
  return convertFileSrc(path);
}

export const api = {
  listAudioDevices: () => invoke<AudioDevice[]>("list_audio_devices"),
  listAudioProcesses: () => invoke<AudioProcess[]>("list_audio_processes"),
  listCaptureTargets: () => invoke<CaptureTarget[]>("list_capture_targets"),
  listEncoders: () => invoke<EncoderInfo[]>("list_encoders"),

  getConfig: () => invoke<AppConfig>("get_config"),
  setConfig: (config: AppConfig) => invoke<void>("set_config", { config }),
  /** Wirft, wenn eine Kombination ungültig oder schon belegt ist. */
  setHotkeys: (saveClip: string, toggleBuffer: string) =>
    invoke<AppConfig>("set_hotkeys", { saveClip, toggleBuffer }),
  /** Wirft, wenn sich der Ordner nicht anlegen oder nicht beschreiben lässt. */
  setClipDir: (dir: string) => invoke<AppConfig>("set_clip_dir", { dir }),
  defaultClipDir: () => invoke<string>("default_clip_dir"),
  /** Hotkeys stilllegen, solange die Einstellungen eine Kombination aufnehmen. */
  suspendHotkeys: () => invoke<void>("suspend_hotkeys"),
  resumeHotkeys: () => invoke<void>("resume_hotkeys"),

  addAudioSource: (source: AudioSource) =>
    invoke<AppConfig>("add_audio_source", { source }),
  updateAudioSource: (source: AudioSource) =>
    invoke<AppConfig>("update_audio_source", { source }),
  removeAudioSource: (id: string) =>
    invoke<AppConfig>("remove_audio_source", { id }),

  startBuffer: () => invoke<void>("start_buffer"),
  stopBuffer: () => invoke<void>("stop_buffer"),
  saveClip: (seconds?: number) => invoke<Clip>("save_clip", { seconds }),
  status: () => invoke<EngineStatus>("engine_status"),

  appVersion: () => invoke<string>("app_version"),
  checkUpdate: () => invoke<UpdateInfo | null>("check_update"),
  installUpdate: () => invoke<void>("install_update"),

  listClips: () => invoke<Clip[]>("list_clips"),
  deleteClip: (id: string) => invoke<void>("delete_clip", { id }),
  revealClip: (id: string) => invoke<void>("reveal_clip", { id }),
  revealPath: (path: string) => invoke<void>("reveal_path", { path }),
  updateClip: (
    id: string,
    meta: { title: string | null; description: string | null; game: string | null },
  ) => invoke<Clip>("update_clip", { id, ...meta }),
  clipTracks: (id: string) => invoke<ClipTrack[]>("clip_tracks", { id }),
  clipWaveform: (id: string) => invoke<string>("clip_waveform", { id }),
  /**
   * Den Clip so schreiben, wie er im Editor steht: Mischung eingerechnet,
   * Zuschnitt ausgeführt. `startMs`/`endMs` beziehen sich auf die Zeitachse der
   * **aktuellen** Datei.
   *
   * Der Clip wird dabei ersetzt — der Player darf ihn währenddessen nicht offen
   * halten, sonst scheitert das Ersetzen unter Windows.
   */
  applyClipEdit: (
    id: string,
    startMs: number,
    endMs: number,
    tracks: TrackMix[],
  ) => invoke<Clip>("apply_clip_edit", { id, startMs, endMs, tracks }),
  /** Den Zuschnitt aufheben: die ganze Aufnahme zurück, Mischung behalten. */
  restoreClipOriginal: (id: string) =>
    invoke<Clip>("restore_clip_original", { id }),
};

export const events = {
  onLevels: (cb: (levels: LevelMap) => void): Promise<UnlistenFn> =>
    listen<LevelMap>("audio-levels", (e) => cb(e.payload)),
  onStatus: (cb: (s: EngineStatus) => void): Promise<UnlistenFn> =>
    listen<EngineStatus>("engine-status", (e) => cb(e.payload)),
  onClipSaved: (cb: (c: Clip) => void): Promise<UnlistenFn> =>
    listen<Clip>("clip-saved", (e) => cb(e.payload)),
  onClipProgress: (cb: (p: ClipProgress) => void): Promise<UnlistenFn> =>
    listen<ClipProgress>("clip-progress", (e) => cb(e.payload)),
  onUpdateAvailable: (cb: (info: UpdateInfo) => void): Promise<UnlistenFn> =>
    listen<UpdateInfo>("update-available", (e) => cb(e.payload)),
  onAudioErrors: (
    cb: (errors: Record<string, string>) => void,
  ): Promise<UnlistenFn> =>
    listen<Record<string, string>>("audio-errors", (e) => cb(e.payload)),
};
