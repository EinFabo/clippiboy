// Muss mit src-tauri/src/model.rs übereinstimmen (serde camelCase).

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

/** Die drei Quellentypen des Mixers. */
export type SourceKind =
  | { type: "outputDevice"; deviceId: string }
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
  /** Eigene Tonspur im MP4 statt in den Hauptmix. */
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
  /** Puffer von selbst einschalten — siehe `onlyBufferInGame`. */
  autoStart: boolean;
  seconds: number;
}

export type OverlayCorner =
  | "topLeft"
  | "topRight"
  | "bottomLeft"
  | "bottomRight";

/** Der Banner über dem Spiel — jede Meldungsart einzeln abschaltbar. */
export interface OverlayConfig {
  enabled: boolean;
  onClipSaved: boolean;
  onBufferToggle: boolean;
  onError: boolean;
  corner: OverlayCorner;
  durationMs: number;
  /** Gerätename des Bildschirms (`\\.\DISPLAY1`); null = primärer. */
  monitor: string | null;
  /** Statt festem Bildschirm dem Vordergrundfenster folgen. */
  followActiveScreen: boolean;
}

export interface AppConfig {
  recording: RecordingConfig;
  buffer: BufferConfig;
  sources: AudioSource[];
  clipDir: string;
  saveClipHotkey: string;
  toggleBufferHotkey: string;
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
  /** Selbst vergebener Name; ohne ihn zeigt die Galerie den Dateinamen. */
  title: string | null;
  description: string | null;
}

/** Eine Tonspur in der Clipdatei. Spur 0 ist der Hauptmix. */
export interface ClipTrack {
  index: number;
  label: string;
  channels: number;
  /** Entpackte Einzeldatei für die Vorschau; Spur 0 braucht keine. */
  previewPath: string | null;
}

/** Wie eine Spur beim Export gewichtet wird. */
export interface TrackMix {
  index: number;
  gainDb: number;
  muted: boolean;
}

export interface ExportRequest {
  clipId: string;
  startMs: number;
  endMs: number;
  tracks: TrackMix[];
  /** Zieldatei; ohne Angabe landet der Export neben dem Ausgangsclip. */
  output?: string | null;
}

export interface ExportResult {
  path: string;
  durationMs: number;
  sizeBytes: number;
}

/** Nutzlast des `export-progress`-Events. */
export interface ExportProgress {
  clipId: string;
  /** 0 bis 1. */
  progress: number;
}

export interface EngineStatus {
  bufferActive: boolean;
  bufferedSeconds: number;
  bufferBytes: number;
  droppedFrames: number;
  encoder: EncoderId | null;
  fps: number;
  /** Zuletzt im Vordergrund erkanntes Spiel. */
  game: string | null;
}

/** Nutzlast des `overlay-banner`-Events (nur im Overlay-Fenster). */
export interface OverlayBanner {
  kind: "clip" | "buffer" | "bufferOff" | "error" | "info";
  title: string;
  detail: string | null;
  thumbPath: string | null;
  durationMs: number;
}

export type LevelMap = Record<string, number>;

/** Was `check_update` über eine neuere Fassung meldet. */
export interface UpdateInfo {
  version: string;
  currentVersion: string;
  notes: string | null;
  date: string | null;
}
