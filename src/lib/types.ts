// Must match src-tauri/src/model.rs (serde camelCase).

export type DeviceKind = "output" | "input";

export interface AudioDevice {
  id: string;
  name: string;
  kind: DeviceKind;
  isDefault: boolean;
}

export interface AudioProcess {
  pid: number;
  name: string;
  exe: string;
}

/** The mixer's source types. */
export type SourceKind =
  /** The detected game — the backend puts the current process in. */
  | { type: "game" }
  | {
      type: "outputDevice";
      deviceId: string;
      /**
       * Record only what no other source records. Several devices may ask for
       * it; each application goes to the first one that does. Optional on
       * purpose — an older config does not carry the field, and a config the
       * backend cannot read is thrown away whole.
       */
      leftoversOnly?: boolean;
    }
  | { type: "inputDevice"; deviceId: string }
  | { type: "process"; pid: number; mode: "include" | "exclude" };

export interface AudioSource {
  id: string;
  label: string;
  kind: SourceKind;
  enabled: boolean;
  gainDb: number;
  muted: boolean;
  solo: boolean;
  /** Its own audio track in the MP4 instead of the main mix. */
  separateTrack: boolean;
}

export type EncoderId = "nvenc" | "amf" | "qsv" | "x264";

export interface EncoderInfo {
  id: EncoderId;
  name: string;
  available: boolean;
  hardware: boolean;
}

export type TargetKind = "monitor" | "window";

export interface CaptureTarget {
  kind: TargetKind;
  id: string;
  title: string;
  width: number;
  height: number;
  isPrimary: boolean;
  /**
   * Refresh rate in hertz — for a window, that of the screen it sits on. `null`
   * when Windows does not report it.
   */
  refreshHz: number | null;
}

export interface RecordingConfig {
  targetKind: TargetKind;
  targetId: string | null;
  width: number;
  height: number;
  fps: number;
  bitrateKbps: number;
  encoder: EncoderId;
  keyframeSeconds: number;
}

export interface BufferConfig {
  /** Switch the buffer on by itself — see `onlyBufferInGame`. */
  autoStart: boolean;
  seconds: number;
}

export type OverlayCorner =
  | "topLeft"
  | "topRight"
  | "bottomLeft"
  | "bottomRight";

/** The banner over the game — every kind of message can be switched off. */
export interface OverlayConfig {
  enabled: boolean;
  onClipSaved: boolean;
  onBufferToggle: boolean;
  onError: boolean;
  onScreenshot: boolean;
  corner: OverlayCorner;
  durationMs: number;
  /** Device name of the screen (`\\.\DISPLAY1`); null = primary. */
  monitor: string | null;
  /** Follow the foreground window instead of a fixed screen. */
  followActiveScreen: boolean;
}

export interface AppConfig {
  recording: RecordingConfig;
  buffer: BufferConfig;
  sources: AudioSource[];
  clipDir: string;
  saveClipHotkey: string;
  toggleBufferHotkey: string;
  screenshotHotkey: string;
  autoStartWithWindows: boolean;
  onlyBufferInGame: boolean;
  overlay: OverlayConfig;
  trayHintShown: boolean;
}

export interface Clip {
  id: string;
  path: string;
  createdAt: number;
  durationMs: number;
  game: string | null;
  width: number;
  height: number;
  sizeBytes: number;
  thumbPath: string | null;
  /** A name given by hand; without it the gallery shows the file name. */
  title: string | null;
  description: string | null;
  /**
   * Marked with the heart — and a category of its own: the file then lives in
   * the `Favorites` folder, while inside the app the clip stays under its game.
   */
  favorite: boolean;
  /** Trim and mix from the editor; `null` means untouched. */
  edit: ClipEdit | null;
  /**
   * Is the untouched recording still beside it? Then this clip is trimmed and
   * can be pulled open again with `restoreClipOriginal`.
   */
  original: ClipOriginal | null;
  /**
   * A still instead of a recording. Everything to do with time — the duration,
   * the trim, the individual tracks, the waveform — does not apply to it, and
   * the core turns those requests away.
   */
  screenshot: boolean;
}

/**
 * What is set on the clip — the state that was written into the file
 * geschrieben wurde.
 */
export interface ClipEdit {
  /**
   * The full range of the **current** file, i.e. `0 .. durationMs`. Since saving,
   * the trim sits in the file itself; where it sat in the original is recorded in
   * {@link ClipOriginal}.
   */
  startMs: number;
  endMs: number;
  /** The levels currently baked into the file's audio track. */
  tracks: TrackMix[];
}

/**
 * The untouched recording of a trimmed clip — and where the delivered
 * Ausschnitt in ihr sitzt.
 *
 * `startMs` is at the same time the offset by which the individual tracks are
 * shifted against the video: the tracks stay untrimmed and are always in
 * Koordinaten des Originals.
 */
export interface ClipOriginal {
  durationMs: number;
  startMs: number;
  endMs: number;
}

/** How far along rewriting a clip is. */
export interface ClipProgress {
  clipId: string;
  /** 0 bis 1. */
  progress: number;
}

/** One individual track of a clip. Track 0 is the main mix. */
export interface ClipTrack {
  index: number;
  label: string;
  channels: number;
  /**
   * The stored individual track. `null` means the clip has only one audio track
   * and that sits in the video file itself — then there is nothing to mix.
   */
  previewPath: string | null;
}

/** How a track is weighted while mixing. */
export interface TrackMix {
  index: number;
  gainDb: number;
  muted: boolean;
}

export interface EngineStatus {
  bufferActive: boolean;
  bufferedSeconds: number;
  bufferBytes: number;
  droppedFrames: number;
  encoder: EncoderId | null;
  fps: number;
  /** Game last detected in the foreground. */
  game: string | null;
}

/** A rectangle in pixels of a screenshot's original. */
export interface ShotRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * One step of a screenshot's annotation, in the order it was drawn.
 *
 * A layer covers what is under it, a blur reads it — so the two cannot be
 * merged into one picture, and their order is what decides what ends up on top.
 */
export type ShotStep =
  | (ShotRect & { kind: "blur"; radius: number; ellipse: boolean })
  | { kind: "layer"; png: number[] };

/** What a screenshot's editor stands on when it opens. */
export interface ShotEdit {
  crop: ShotRect | null;
  /** The marks as the editor keeps them — opaque to the core. */
  marks: string;
  /** The original with the crop but without the marks: what is drawn on. */
  basePath: string | null;
  /** The untouched picture: what is cropped from. */
  originalPath: string | null;
  originalWidth: number;
  originalHeight: number;
}

/** Payload of the `overlay-banner` event (overlay window only). */
export interface OverlayBanner {
  kind: "clip" | "buffer" | "bufferOff" | "error" | "info" | "screenshot";
  title: string;
  detail: string | null;
  thumbPath: string | null;
  durationMs: number;
}

export type LevelMap = Record<string, number>;

/** What `check_update` reports about a newer version. */
export interface UpdateInfo {
  version: string;
  currentVersion: string;
  notes: string | null;
  date: string | null;
}
