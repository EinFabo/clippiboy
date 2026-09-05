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
  StorageUsage,
  TrackMix,
  UpdateInfo,
  ShotEdit,
  ShotRect,
  ShotStep,
} from "./types";

/** Is the app running inside the Tauri container? (Plain browser Vite has no IPC.) */
export const inTauri = "__TAURI_INTERNALS__" in window;

/// Translate a local file into a URL the WebView can load.
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
  /** Throws when a combination is invalid or already taken. */
  /** All three at once — the core only accepts them together. */
  setHotkeys: (saveClip: string, toggleBuffer: string, screenshot: string) =>
    invoke<AppConfig>("set_hotkeys", { saveClip, toggleBuffer, screenshot }),
  /** Throws when the folder cannot be created or written to. */
  setClipDir: (dir: string) => invoke<AppConfig>("set_clip_dir", { dir }),
  defaultClipDir: () => invoke<string>("default_clip_dir"),
  /** Suspend the hotkeys while the settings record a combination. */
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
  takeScreenshot: () => invoke<Clip>("take_screenshot"),
  status: () => invoke<EngineStatus>("engine_status"),

  appVersion: () => invoke<string>("app_version"),
  /** Throw the Stream Deck token away and generate a new one. */
  regenerateControlToken: () => invoke<AppConfig>("regenerate_control_token"),
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
  /**
   * Put the video file on the clipboard — not the path, the file. In Discord,
   * Ctrl+V then attaches the clip.
   */
  copyClipFile: (id: string) => invoke<void>("copy_clip_file", { id }),
  /** Screenshots only: the picture itself, not the file. */
  copyClipImage: (id: string) => invoke<void>("copy_clip_image", { id }),
  /**
   * Write a screenshot from its original, its marks and its crop.
   *
   * Everything is rebuilt from the untouched picture every time — that is what
   * lets the crop be taken back without losing the marks, and the other way
   * round. Marks and crop are both in pixels of the **original**.
   *
   * A layer is a PNG with alpha at the original's size; only those travel,
   * never the finished picture, so they weigh kilobytes. A blur is an area
   * instead, because a layer can only cover while blurring has to read what is
   * under it — and the order between the two decides what ends up on top.
   */
  writeScreenshot: (
    id: string,
    crop: ShotRect | null,
    steps: ShotStep[],
    marks: string,
  ) => invoke<Clip>("write_screenshot", { id, crop, steps, marks }),
  screenshotEdit: (id: string) => invoke<ShotEdit>("screenshot_edit", { id }),
  screenshotHasOriginal: (id: string) =>
    invoke<boolean>("screenshot_has_original", { id }),
  /** Open the clip in the Windows default player. */
  openClip: (id: string) => invoke<void>("open_clip", { id }),
  clipboardWriteText: (text: string) =>
    invoke<void>("clipboard_write_text", { text }),
  /** An empty string means there is no text on the clipboard. */
  clipboardReadText: () => invoke<string>("clipboard_read_text"),
  /** Set or take away the heart. The file stays where it is. */
  setClipFavorite: (id: string, favorite: boolean) =>
    invoke<Clip>("set_clip_favorite", { id, favorite }),
  /**
   * Move the file into the folder it belongs in (game or `Favorites`).
   * Deliberately separate from editing: while the clip is playing in the player,
   * nobody may pull its file out from under it.
   */
  fileClip: (id: string) => invoke<Clip>("file_clip", { id }),
  clipTracks: (id: string) => invoke<ClipTrack[]>("clip_tracks", { id }),
  clipWaveform: (id: string) => invoke<string>("clip_waveform", { id }),
  /**
   * Write the clip exactly as it stands in the editor: mix applied, trim carried
   * out. `startMs`/`endMs` refer to the **current** file's timeline.
   *
   * The clip is replaced in the process — the player must not hold it open while
   * that happens, or the replace fails on Windows.
   */
  applyClipEdit: (
    id: string,
    startMs: number,
    endMs: number,
    tracks: TrackMix[],
  ) => invoke<Clip>("apply_clip_edit", { id, startMs, endMs, tracks }),
  /** Undo the trim: the whole recording back, keep the mix. */
  restoreClipOriginal: (id: string) =>
    invoke<Clip>("restore_clip_original", { id }),
  /**
   * Throw the untouched recording away and keep the trimmed clip. Undo is gone
   * afterwards; the trim stays, and so does the mix.
   */
  discardClipOriginal: (id: string) =>
    invoke<Clip>("discard_clip_original", { id }),
  /** What the originals, tracks and thumbnails occupy in the app data folder. */
  storageUsage: () => invoke<StorageUsage>("storage_usage"),
  /**
   * Write a copy of the clip that comes in under `targetBytes`. The clip itself
   * is not touched — this is a second file at `output`.
   */
  exportClip: (id: string, targetBytes: number, output: string) =>
    invoke<void>("export_clip", { id, targetBytes, output }),
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
  /** Sources that do run, but not the way one expects. */
  onAudioWarnings: (
    cb: (warnings: Record<string, string>) => void,
  ): Promise<UnlistenFn> =>
    listen<Record<string, string>>("audio-warnings", (e) => cb(e.payload)),
  /**
   * Which processes each source is tapping. For the leftovers there is no
   * device to look at, so this is the only way to see what is in the track.
   */
  onAudioTaps: (
    cb: (taps: Record<string, number[]>) => void,
  ): Promise<UnlistenFn> =>
    listen<Record<string, number[]>>("audio-taps", (e) => cb(e.payload)),
};
