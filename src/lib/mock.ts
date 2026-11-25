// Fallback data so the UI can be developed in the browser without the Tauri
// backend (`npm run dev`). None of it is used in a real build.
import type {
  AppConfig,
  AudioDevice,
  AudioProcess,
  CameraDevice,
  CaptureTarget,
  CaptureThumb,
  Clip,
  ClipTrack,
  EncoderInfo,
  Trashed,
} from "./types";

export const mockDevices: AudioDevice[] = [
  { id: "out-1", name: "Headphones (Realtek USB Audio)", kind: "output", isDefault: true },
  { id: "out-2", name: "LG HDR 4K (NVIDIA High Definition Audio)", kind: "output", isDefault: false },
  { id: "in-1", name: "Mikrofon (Elgato Wave XLR)", kind: "input", isDefault: true },
  { id: "in-2", name: "Line In (Focusrite Scarlett)", kind: "input", isDefault: false },
];

export const mockCameras: CameraDevice[] = [
  { id: "\\\\?\\usb#vid_046d&pid_08e5#c920", name: "HD Pro Webcam C920" },
  { id: "\\\\?\\usb#vid_328f&pid_006d#c960", name: "EMEET SmartCam C960" },
];

export const mockProcesses: AudioProcess[] = [
  { pid: 4321, name: "Discord", exe: "Discord.exe", count: 2, active: true },
  { pid: 8899, name: "Counter-Strike 2", exe: "cs2.exe" },
  { pid: 1122, name: "Spotify", exe: "Spotify.exe" },
  { pid: 7654, name: "Chrome", exe: "chrome.exe" },
];

export const mockTargets: CaptureTarget[] = [
  { kind: "monitor", id: "\\\\.\\DISPLAY1", stableId: "\\\\?\\DISPLAY#ACME1234#1#{guid}", title: "E2212F — 2560×1440", width: 2560, height: 1440, isPrimary: true, refreshHz: 165, x: 0, y: 0, icon: null, isGame: false },
  { kind: "monitor", id: "\\\\.\\DISPLAY2", stableId: "\\\\?\\DISPLAY#ACME5678#1#{guid}", title: "CR270E — 1920×1080", width: 1920, height: 1080, isPrimary: false, refreshHz: 60, x: -1920, y: 180, icon: null, isGame: false },
  { kind: "window", id: "0x00120A", stableId: null, title: "Counter-Strike 2", width: 2560, height: 1440, isPrimary: false, refreshHz: 165, x: 0, y: 0, icon: null, isGame: true },
  { kind: "window", id: "0x00340C", stableId: null, title: "Discord", width: 1280, height: 860, isPrimary: false, refreshHz: 165, x: 0, y: 0, icon: null, isGame: false },
];

/** Stand-in stills for the monitor map: a game on the first screen, a desktop on the second. */
const stillSvg: Record<string, string> = {
  "\\\\.\\DISPLAY1": `<svg xmlns="http://www.w3.org/2000/svg" width="320" height="180">
    <defs>
      <radialGradient id="sky" cx="25%" cy="20%" r="60%"><stop offset="0" stop-color="#4a6fa8"/><stop offset="1" stop-color="#4a6fa8" stop-opacity="0"/></radialGradient>
      <radialGradient id="fire" cx="65%" cy="85%" r="45%"><stop offset="0" stop-color="#d9822b"/><stop offset="1" stop-color="#d9822b" stop-opacity="0"/></radialGradient>
    </defs>
    <rect width="320" height="180" fill="#1b2130"/><rect width="320" height="180" fill="url(#sky)"/><rect width="320" height="180" fill="url(#fire)"/>
    <path d="M0 132 L70 104 L130 122 L210 92 L320 118 L320 180 L0 180Z" fill="#11141c" opacity=".85"/>
  </svg>`,
  "\\\\.\\DISPLAY2": `<svg xmlns="http://www.w3.org/2000/svg" width="320" height="180">
    <defs><linearGradient id="wall" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#2a2140"/><stop offset="1" stop-color="#101018"/></linearGradient></defs>
    <rect width="320" height="180" fill="url(#wall)"/>
    <rect x="28" y="22" width="150" height="104" rx="6" fill="#2b2b36"/><rect x="28" y="22" width="150" height="14" rx="6" fill="#3a3a48"/>
    <rect x="150" y="58" width="140" height="92" rx="6" fill="#23232d"/><rect x="150" y="58" width="140" height="14" rx="6" fill="#5865f2" opacity=".8"/>
    <rect y="168" width="320" height="12" fill="#0c0c12"/>
  </svg>`,
};

/** What `capture_thumbs` answers in the browser: the stand-ins above, nothing for anything else. */
export function mockThumbs(ids: string[]): CaptureThumb[] {
  return ids.map((id) => ({
    id,
    jpeg: stillSvg[id] ? `data:image/svg+xml,${encodeURIComponent(stillSvg[id])}` : null,
  }));
}

export const mockEncoders: EncoderInfo[] = [
  { id: "nvenc", name: "NVIDIA NVENC (H.264)", available: true, hardware: true },
  { id: "amf", name: "AMD AMF (H.264)", available: false, hardware: true },
  { id: "qsv", name: "Intel QuickSync (H.264)", available: false, hardware: true },
  { id: "x264", name: "x264 (CPU, Fallback)", available: true, hardware: false },
];

export const mockConfig: AppConfig = {
  recording: {
    targetKind: "monitor",
    targetId: "\\\\.\\DISPLAY1",
    targetStableId: "\\\\?\\DISPLAY#ACME1234#1#{guid}",
    targetRect: { x: 0, y: 0, width: 2560, height: 1440 },
    width: 1920,
    height: 1080,
    fps: 60,
    bitrateKbps: 19000,
    quality: 70,
    encoder: "nvenc",
    keyframeSeconds: 2,
  },
  buffer: { autoStart: false, seconds: 120, clipSeconds: 0, memoryMb: 0 },
  sources: [
    {
      id: "src-game",
      label: "Game",
      kind: { type: "process", pid: 8899, exe: "cs2.exe", mode: "include" },
      enabled: true,
      gainDb: 0,
      muted: false,
      solo: false,
      separateTrack: false,
    },
    {
      id: "src-discord",
      label: "Discord",
      kind: { type: "process", pid: 4321, exe: "Discord.exe", mode: "include" },
      enabled: true,
      gainDb: -3,
      muted: false,
      solo: false,
      separateTrack: true,
    },
    {
      id: "src-mic",
      label: "Mikrofon",
      kind: { type: "inputDevice", deviceId: "in-1" },
      enabled: true,
      gainDb: 2,
      muted: false,
      solo: false,
      separateTrack: true,
    },
  ],
  clipDir: "C:\\Users\\user\\Videos\\ClippiBoy",
  saveClipHotkey: "Ctrl+Shift+S",
  toggleBufferHotkey: "Ctrl+Shift+B",
  screenshotHotkey: "Ctrl+Shift+P",
  recordHotkey: "Ctrl+Shift+R",
  consoleHotkey: "Alt+C",
  saveClipHotkeyAlt: "",
  toggleBufferHotkeyAlt: "",
  screenshotHotkeyAlt: "",
  recordHotkeyAlt: "",
  consoleHotkeyAlt: "",
  padProfiles: {
    all: { saveClip: "", toggleBuffer: "", screenshot: "", record: "", console: "" },
    xbox: null,
    playstation: null,
    nintendo: null,
  },
  padFamiliesSeen: [],
  padLastUsed: null,
  consoleEnabled: true,
  consoleScale: 1,
  consoleMonitor: null,
  consoleMonitorStableId: null,
  consoleFollowActiveScreen: true,
  consoleInCapture: false,
  consoleGlow: true,
  consoleStyle: "dock",
  consoleTakeFocus: false,
  autoStartWithWindows: false,
  onlyBufferInGame: true,
  overlay: {
    enabled: true,
    onClipSaved: true,
    onBufferToggle: true,
    onError: true,
    onScreenshot: true,
    onRecording: true,
    recBadge: true,
    corner: "bottomRight",
    durationMs: 3500,
    monitor: null,
    monitorStableId: null,
    followActiveScreen: false,
  },
  trayHintShown: false,
  padButtonHintShown: false,
  guideHintShown: false,
  discordHintShown: false,
  watermark: true,
  setupDone: true,
  usageStats: true,
  usageStatsAsked: true,
  control: { enabled: true, port: 47653, token: "mock-token" },
  accentColor: null,
  colorsUnlocked: false,
  accentMode: "solid",
  accentColor2: null,
  rgbSpeed: "slow",
  friends: {
    invisible: false,
    shareGame: true,
    nameOnLinks: true,
    friendOnLinks: true,
    notifyRequests: true,
    notifyOnline: false,
    notifyGames: true,
    notifyGamesFavoritesOnly: false,
    notifyClips: true,
    notifyWhilePlaying: false,
    status: "",
    busy: false,
    favorites: [],
    acceptClips: "all",
    tagFriends: true,
    discordStatus: true,
  },
  sounds: {
    enabled: true,
    volume: 0.5,
    device: null,
    deviceName: null,
    clipSaved: { on: true, volume: 1 },
    screenshot: { on: true, volume: 1 },
    recordingStarted: { on: true, volume: 1 },
    recordingSaved: { on: true, volume: 1 },
    bufferOn: { on: true, volume: 1 },
    bufferOff: { on: true, volume: 1 },
    friendOnline: { on: true, volume: 1 },
    clipReceived: { on: true, volume: 1 },
    error: { on: true, volume: 1 },
  },
  facecam: {
    enabled: true,
    deviceId: null,
    deviceName: null,
    resolution: "p720",
    fps: 30,
    layout: { visible: true, x: 1, y: 1, size: 0.25, shape: "rounded", mirror: false },
    portrait: { split: 0.35, gameX: 0.5 },
    delayMs: -40,
    inClips: true,
    inRecordings: true,
    inScreenshots: true,
    consolePreview: true,
    virtualCamera: false,
  },
  discordLimit: "free",
  muteDiscordOnShare: false,
  deleteOlderThanDays: 0,
  streamerMode: "off",
  webhookAutoPost: false,
  revision: 0,
};

export const mockClips: Clip[] = [
  {
    id: "c1",
    path: "C:\\Users\\user\\Videos\\ClippiBoy\\cs2_2026-08-17_22-14-03.mp4",
    createdAt: Date.now() - 1000 * 60 * 12,
    durationMs: 32_000,
    game: "Counter-Strike 2",
    width: 1920,
    height: 1080,
    sizeBytes: 168_000_000,
    thumbPath: null,
    title: "Ace on Mirage",
    description: "Last round, 1v3 after the retake.",
    favorite: true,
    edit: null,
    original: null,
    originalAvailable: false,

    facecam: { layout: { visible: true, x: 1, y: 1, size: 0.25, shape: "rounded", mirror: false }, portrait: { split: 0.35, gameX: 0.5 }, delayMs: -40, state: "pending" },
    screenshot: false,
    recording: false,
    tags: ["ace", "clutch"],
  },
  {
    id: "c2",
    path: "C:\\Users\\user\\Videos\\ClippiBoy\\cs2_2026-08-17_21-48-51.mp4",
    createdAt: Date.now() - 1000 * 60 * 60 * 3,
    durationMs: 60_000,
    game: "Counter-Strike 2",
    width: 1920,
    height: 1080,
    sizeBytes: 305_000_000,
    thumbPath: null,
    title: null,
    description: null,
    favorite: false,
    edit: null,
    original: null,
    originalAvailable: false,

    facecam: { layout: { visible: true, x: 0, y: 1, size: 0.25, shape: "circle", mirror: false }, portrait: { split: 0.35, gameX: 0.5 }, delayMs: -40, state: "ready" },
    screenshot: false,
    recording: false,
    tags: ["clutch"],
  },
  {
    id: "c3",
    path: "C:\\Users\\user\\Videos\\ClippiBoy\\nohesi_2026-08-16_02-11-20.mp4",
    createdAt: Date.now() - 1000 * 60 * 60 * 26,
    durationMs: 18_500,
    game: "No Hesi",
    width: 1920,
    height: 1080,
    sizeBytes: 94_000_000,
    thumbPath: null,
    title: null,
    description: null,
    favorite: false,
    edit: null,
    original: null,
    originalAvailable: false,

    facecam: null,
    screenshot: false,
    recording: false,
    tags: [],
  },
  // The gallery has to survive the bad case too: one clip with no game and three
  // misdetections from before the stricter title check — that is what shows
  // whether the filter bar truncates, scrolls and can be tidied up. The German
  // window titles are deliberate: they come from a German-locale Windows.
  {
    id: "c4",
    path: "C:\\Users\\user\\Videos\\ClippiBoy\\clip_2026-08-15_19-02-44.mp4",
    createdAt: Date.now() - 1000 * 60 * 60 * 30,
    durationMs: 24_000,
    game: null,
    width: 2560,
    height: 1440,
    sizeBytes: 142_000_000,
    thumbPath: null,
    title: null,
    description: null,
    favorite: false,
    edit: null,
    original: null,
    originalAvailable: false,

    facecam: null,
    screenshot: false,
    recording: false,
    tags: [],
  },
  ...[
    "(102) WIR MÜSSEN PAYEN - YouTube – Opera",
    "C:\\Users\\user\\projects\\clippiboy\\synctest.mp4",
    "Snipping Tool Überlagerung",
  ].map((game, i) => ({
    id: `c${5 + i}`,
    path: `C:\\Users\\user\\Videos\\ClippiBoy\\clip_2026-08-1${i}_08-30-00.mp4`,
    createdAt: Date.now() - 1000 * 60 * 60 * (40 + i * 5),
    durationMs: 12_000 + i * 3000,
    game,
    width: 1920,
    height: 1080,
    sizeBytes: 60_000_000,
    thumbPath: null,
    title: null,
    description: null,
    favorite: false,
    edit: null,
    original: null,
    originalAvailable: false,

    facecam: null,
    screenshot: false,
    recording: false,
    tags: [],
  })),
  ...["Counter-Strike 2", null].map((game, i) => ({
    id: `s${1 + i}`,
    path: `C:\\Users\\user\\Videos\\ClippiBoy\\shot_2026-08-2${i}_19-04-1${i}.png`,
    createdAt: Date.now() - 1000 * 60 * (30 + i * 90),
    // A still has no length, and the gallery shows the camera instead.
    durationMs: 0,
    game,
    width: 3840,
    height: 2160,
    sizeBytes: 9_400_000 - i * 1_200_000,
    thumbPath: null,
    title: i === 0 ? "Scoreboard" : null,
    description: null,
    favorite: false,
    edit: null,
    original: null,
    originalAvailable: false,

    facecam: null,
    screenshot: true,
    recording: false,
    tags: [],
  })),
  // More days, games, tags and two recordings: enough for the day headings
  // ("Today", a weekday, a date, last year) and for a chip row that overflows.
  ...(
    [
      ["r1", "Counter-Strike 2", 4 * 60, 1_082_000, 2_100_000_000, true, ["ranked"], "Premier evening"],
      ["c8", "Valorant", 2 * 24 * 60, 41_000, 210_000_000, false, ["clutch", "funny"], null],
      ["r2", "Valorant", 3 * 24 * 60, 2_700_000, 3_400_000_000, true, ["ranked"], null],
      ["c9", "Rocket League", 5 * 24 * 60, 28_000, 120_000_000, false, ["funny"], "Ceiling shot"],
      ["c10", "Minecraft", 12 * 24 * 60, 55_000, 260_000_000, false, [], null],
      ["c11", "Valorant", 400 * 24 * 60, 33_000, 150_000_000, false, ["ace"], "First ace"],
    ] as const
  ).map(([id, game, minutesAgo, durationMs, sizeBytes, recording, tags, title]) => ({
    id,
    path: `C:\\Users\\user\\Videos\\ClippiBoy\\${id}.mp4`,
    createdAt: Date.now() - 1000 * 60 * minutesAgo,
    durationMs,
    game,
    width: 2560,
    height: 1440,
    sizeBytes,
    thumbPath: null,
    title,
    description: null,
    favorite: id === "c11",
    edit: null,
    original: null,
    originalAvailable: false,
    facecam: null,
    screenshot: false,
    recording,
    tags: [...tags],
  })),
  {
    id: "s3",
    path: "C:\\Users\\user\\Videos\\ClippiBoy\\shot_elden.png",
    createdAt: Date.now() - 1000 * 60 * 60 * 24 * 9,
    durationMs: 0,
    game: "Elden Ring",
    width: 3840,
    height: 2160,
    sizeBytes: 8_100_000,
    thumbPath: null,
    title: null,
    description: null,
    favorite: false,
    edit: null,
    original: null,
    originalAvailable: false,
    facecam: null,
    screenshot: true,
    recording: false,
    tags: [],
  },
];

/** Recently deleted, so the trash button and its view show without a backend. */
export const mockTrash: Trashed[] = [mockClips[2], mockClips[3]].map((clip, i) => ({
  clip: { ...clip, id: `t${i + 1}` },
  deletedAt: Date.now() - 1000 * 60 * 60 * 24 * (i + 1),
  expiresAt: Date.now() + 1000 * 60 * 60 * 24 * (29 - i * 26),
  fromPath: clip.path,
}));

/** Without a backend there are no real audio tracks — the mixer should still be there. */
export const mockTracks: ClipTrack[] = [
  { index: 0, label: "Main mix", channels: 2, previewPath: "mock/0.m4a" },
  { index: 1, label: "Discord", channels: 2, previewPath: "mock/1.m4a" },
  { index: 2, label: "Mikrofon", channels: 2, previewPath: "mock/2.m4a" },
];
