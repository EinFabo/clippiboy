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
  | { type: "outputDevice"; deviceId: string }
  | { type: "inputDevice"; deviceId: string }
  | {
      type: "process";
      pid: number;
      /**
       * The exe behind that pid. A pid dies with the application; this is what
       * lets the backend find the source again after a restart.
       */
      exe: string | null;
      mode: "include" | "exclude";
    };

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

/** How the encoder decides how many bits a frame is worth. */
export type RateControl =
  | "quality"
  | "peakConstrainedVbr"
  | "unconstrainedVbr";

export interface EncoderInfo {
  id: EncoderId;
  name: string;
  available: boolean;
  hardware: boolean;
}

export type TargetKind = "monitor" | "window";

/** Die Gestalt des Docks über dem Spiel. Muss zu `ConsoleStyle` in `model.rs`
    passen — der Kern reicht den Wert nur durch, gezeichnet wird er hier. */
export type ConsoleStyle = "dock" | "pill" | "radial";

/** How the accent is laid on. Has to match `AccentMode` in `model.rs`. */
export type AccentMode = "solid" | "gradient" | "rgb";

/** How long one turn of the hue takes in RGB. Matches `RgbSpeed` in `model.rs`. */
export type RgbSpeed = "slow" | "medium";

/** Everything about the accent, as `accent-changed` carries it (`Accent` in `model.rs`). */
export interface Accent {
  mode: AccentMode;
  color: string | null;
  color2: string | null;
  speed: RgbSpeed;
}

export interface CaptureTarget {
  kind: TargetKind;
  id: string;
  /**
   * A monitor's identity that survives a reboot — the panel's device interface
   * path. `id` is the name Windows hands out by enumeration order
   * (`\\.\DISPLAY2`), and that can point at a different screen after the next
   * boot. `null` for windows, and for a monitor Windows will not describe.
   */
  stableId: string | null;
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
  /** The chosen monitor's stable identity; wins over `targetId`. */
  targetStableId: string | null;
  width: number;
  height: number;
  fps: number;
  /**
   * Derived from resolution and frame rate, not set by hand. With constant
   * quality the encoder ignores it — it still sizes the memory budget and still
   * governs on an encoder that refuses quality mode.
   */
  bitrateKbps: number;
  /** What the picture is worth, 1 (smallest) to 100 (best). The encoder's own default is 70. */
  quality: number;
  encoder: EncoderId;
  keyframeSeconds: number;
}

export interface BufferConfig {
  /** Switch the buffer on by itself — see `onlyBufferInGame`. */
  autoStart: boolean;
  seconds: number;
  /**
   * How much of the buffer a save writes. `0` means all of it — the behaviour
   * of every version before this setting, and therefore what an existing config
   * keeps doing.
   */
  clipSeconds: number;
  /**
   * Ceiling on what the packet ring may occupy, in megabytes. `0` derives it
   * from bitrate and buffer length.
   */
  memoryMb: number;
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
  onRecording: boolean;
  /** The REC badge that stands for the whole recording. */
  recBadge: boolean;
  corner: OverlayCorner;
  durationMs: number;
  /** Device name of the screen (`\\.\DISPLAY1`); null = primary. */
  monitor: string | null;
  /** That screen's stable identity; wins over `monitor`. */
  monitorStableId: string | null;
  /** Follow the foreground window instead of a fixed screen. */
  followActiveScreen: boolean;
}

/**
 * The local port a Stream Deck presses ClippiBoy's buttons through. Listens on
 * 127.0.0.1 only, and every request has to carry the token.
 */
export interface ControlConfig {
  enabled: boolean;
  port: number;
  token: string;
}

/**
 * Was „In der App öffnen" aus der Konsole über dem Spiel mitbringt: der Clip und
 * die Sekunde, an der dort gerade geschaut wurde.
 */
export interface FocusClip {
  id: string;
  at: number;
}

export interface AppConfig {
  recording: RecordingConfig;
  buffer: BufferConfig;
  sources: AudioSource[];
  clipDir: string;
  saveClipHotkey: string;
  toggleBufferHotkey: string;
  screenshotHotkey: string;
  /** Start and stop a recording. */
  recordHotkey: string;
  /** Open the console over the game. */
  consoleHotkey: string;
  /** Whether that hotkey does anything at all. */
  consoleEnabled: boolean;
  /** How big the console is drawn, 0.8 to 1.6. */
  consoleScale: number;
  /** Device name of the screen (`\\.\DISPLAY1`); null = primary. */
  consoleMonitor: string | null;
  consoleMonitorStableId: string | null;
  /** Opens on whichever screen has the focus. Beats the two above. */
  consoleFollowActiveScreen: boolean;
  /** Visible to Discord and any other screen recording — and then also inside a
      clip saved while it is open. */
  consoleInCapture: boolean;
  /** The violet wash rising from the lower corners of the screen while the
      console is open. It tints the game underneath, so it can be switched off. */
  consoleGlow: boolean;
  /** Die Gestalt des Docks — dieselben Knöpfe, anders angeordnet.
      `dock` ist die beschriftete Leiste unten, `pill` das runde Dock ohne
      Beschriftung, `radial` die Aktionen im Kreis mit dem Panel rechts. */
  consoleStyle: ConsoleStyle;
  autoStartWithWindows: boolean;
  onlyBufferInGame: boolean;
  overlay: OverlayConfig;
  trayHintShown: boolean;
  control: ControlConfig;
  /** An accent of one's own as `#rrggbb`; `null` is the violet. */
  accentColor: string | null;
  /** Whether the Colors tab is shown in the settings. */
  colorsUnlocked: boolean;
  accentMode: AccentMode;
  /** The second colour of the gradient, `#rrggbb`; `null` is the violet. */
  accentColor2: string | null;
  rgbSpeed: RgbSpeed;
  friends: FriendsConfig;
}

/** What friends see of you, and what you hear about them. */
export interface FriendsConfig {
  invisible: boolean;
  shareGame: boolean;
  notifyRequests: boolean;
  notifyOnline: boolean;
  notifyGames: boolean;
  notifyWhilePlaying: boolean;
  /** A line of one's own under the name; empty for none. */
  status: string;
  /** Do not disturb: no notices, and clips sent meanwhile are turned down. */
  busy: boolean;
  /** Friend ids pinned to the top — kept only on this PC. */
  favorites: string[];
  acceptClips: AcceptClips;
  /** Friends in the same game go onto a new clip as tags ("with Luca"). */
  tagFriends: boolean;
}

/** Whose clips may be offered at all. */
export type AcceptClips = "all" | "favorites" | "off";

export interface FriendUser {
  id: string;
  username: string;
  displayName: string;
  avatar: string | null;
}

export interface FriendMe extends FriendUser {
  friendCode: string;
  allowRequests: boolean;
}

export interface FriendPending extends FriendUser {
  /** When the request was made, ms since the epoch. */
  since: number;
}

export interface FriendPresence {
  game: string | null;
  /** When the game started, ms since the epoch. */
  since: number | null;
  status: string | null;
  busy: boolean;
}

/** Everything the Friends page draws — the core sends it whole on every change. */
export interface FriendsView {
  signedIn: boolean;
  signingIn: boolean;
  connected: boolean;
  me: FriendMe | null;
  lists: {
    friends: FriendUser[];
    incoming: FriendPending[];
    outgoing: FriendPending[];
    blocked: FriendUser[];
  };
  /** Online friends by id; missing means offline. */
  presence: Record<string, FriendPresence>;
  /** When an offline friend was last seen, ms since the epoch. */
  lastSeen: Record<string, number>;
}

/** A clip on its way to or from a friend — see `share.rs`. */
export interface Transfer {
  id: string;
  direction: "out" | "in";
  friendId: string;
  friendName: string;
  name: string;
  size: number;
  moved: number;
  stage: "preparing" | "asking" | "moving" | "done" | "declined" | "expired" | "cancelled" | "failed";
  error: string | null;
  /** Out: the clip sent. In: the clip in one's own library once it is in. */
  clipId: string | null;
  game: string | null;
  durationMs: number;
  screenshot: boolean;
  recording: boolean;
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
   * Is the untouched recording still on disk, so the trim can be undone?
   *
   * A separate question from `original` above: that record outlives the file,
   * because the individual tracks are stored in its coordinates.
   */
  originalAvailable: boolean;
  /**
   * A still instead of a recording. Everything to do with time — the duration,
   * the trim, the individual tracks, the waveform — does not apply to it, and
   * the core turns those requests away.
   */
  screenshot: boolean;
  /**
   * Started and stopped by hand instead of cut out of the buffer. Has a chip
   * and a folder of its own, and can run for an hour.
   */
  recording: boolean;
  /**
   * Free labels, set in the player. The core keeps each spelling once
   * regardless of case and sorts them; the gallery filters by one at a time.
   */
  tags: string[];
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

/**
 * One individual track of a clip. The main mix comes first, then the sources
 * that were given their own track — unless nothing ever ran into the main mix,
 * in which case the clip has none and the first track is a source.
 */
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

/** What ClippiBoy keeps out of sight in the app data directory. */
export interface StorageUsage {
  /** The clips, screenshots and recordings themselves. */
  clipsBytes: number;
  originalsBytes: number;
  tracksBytes: number;
  thumbsBytes: number;
  /** The web views' own folders. */
  cacheBytes: number;
  /** ffmpeg, downloaded on the first start. */
  toolsBytes: number;
}

/** Untouched recordings kept to undo trims — what "Clear all trims" frees. */
export interface TrimOriginals {
  count: number;
  bytes: number;
}

export interface EngineStatus {
  bufferActive: boolean;
  bufferedSeconds: number;
  bufferBytes: number;
  droppedFrames: number;
  encoder: EncoderId | null;
  /** What the encoder really does about bitrate. `null` until one is running. */
  rateControl: RateControl | null;
  fps: number;
  /** Game last detected in the foreground. */
  game: string | null;
  /**
   * How long that game has been up, in seconds; `null` when none is running.
   *
   * Counts the process, not the foreground — alt-tabbing to the browser does
   * not end a session and coming back does not start a second one.
   */
  gameSeconds: number | null;
  /** What is still free on the drive the clips go to; `null` when unknown. */
  freeBytes: number | null;
  /** A recording started by hand is running. */
  recording: boolean;
  /** How long it runs so far. */
  recordingSeconds: number;
  /** What it has written to disk so far. */
  recordingBytes: number;
  /** The chosen screen is not connected and capture runs on this one instead —
      its name. `null` while it runs where it was told to. */
  screenFallback: string | null;
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
  kind: "clip" | "buffer" | "bufferOff" | "error" | "info" | "screenshot" | "recording" | "friend" | "friendOnline" | "friendGame";
  title: string;
  detail: string | null;
  thumbPath: string | null;
  durationMs: number;
  /** A friend's picture, for the friend notices. */
  avatar?: string | null;
  /** The screen corner the banner stands in — the small notices hug its edge. */
  corner?: "topLeft" | "topRight" | "bottomLeft" | "bottomRight";
}

export type LevelMap = Record<string, number>;

/** What `check_update` reports about a newer version. */
export interface UpdateInfo {
  version: string;
  currentVersion: string;
  notes: string | null;
  date: string | null;
}

/** Where ffmpeg stands — fetched once on first start. */
export type FfmpegStatus =
  | { state: "checking" | "unpacking" | "ready" }
  | { state: "downloading"; downloaded: number; total: number | null }
  | { state: "failed"; error: string; retryInSecs: number };

/** How far the download of an update has come. */
export interface UpdateProgress {
  downloaded: number;
  /** `null` when the server does not say how large the package is. */
  total: number | null;
  /** The package is in; the installer is being started. */
  finished: boolean;
}
