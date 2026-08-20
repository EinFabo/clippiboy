// Fallback-Daten, damit die UI auch ohne Tauri-Backend im Browser entwickelt
// werden kann (`npm run dev`). Im echten Build wird nichts davon benutzt.
import type {
  AppConfig,
  AudioDevice,
  AudioProcess,
  CaptureTarget,
  Clip,
  ClipTrack,
  EncoderInfo,
} from "./types";

export const mockDevices: AudioDevice[] = [
  { id: "out-1", name: "Kopfhörer (Realtek USB Audio)", kind: "output", isDefault: true },
  { id: "out-2", name: "LG HDR 4K (NVIDIA High Definition Audio)", kind: "output", isDefault: false },
  { id: "in-1", name: "Mikrofon (Elgato Wave XLR)", kind: "input", isDefault: true },
  { id: "in-2", name: "Line In (Focusrite Scarlett)", kind: "input", isDefault: false },
];

export const mockProcesses: AudioProcess[] = [
  { pid: 4321, name: "Discord", exe: "Discord.exe" },
  { pid: 8899, name: "Counter-Strike 2", exe: "cs2.exe" },
  { pid: 1122, name: "Spotify", exe: "Spotify.exe" },
  { pid: 7654, name: "Chrome", exe: "chrome.exe" },
];

export const mockTargets: CaptureTarget[] = [
  { kind: "monitor", id: "\\\\.\\DISPLAY1", title: "Monitor 1 — 2560×1440", width: 2560, height: 1440, isPrimary: true },
  { kind: "monitor", id: "\\\\.\\DISPLAY2", title: "Monitor 2 — 1920×1080", width: 1920, height: 1080, isPrimary: false },
  { kind: "window", id: "0x00120A", title: "Counter-Strike 2", width: 2560, height: 1440, isPrimary: false },
];

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
    width: 1920,
    height: 1080,
    fps: 60,
    bitrateKbps: 40000,
    encoder: "nvenc",
    keyframeSeconds: 2,
  },
  buffer: { autoStart: false, seconds: 120 },
  sources: [
    {
      id: "src-game",
      label: "Spiel",
      kind: { type: "process", pid: 8899, mode: "include" },
      enabled: true,
      gainDb: 0,
      muted: false,
      solo: false,
      separateTrack: false,
    },
    {
      id: "src-discord",
      label: "Discord",
      kind: { type: "process", pid: 4321, mode: "include" },
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
  clipDir: "C:\\Users\\fabia\\Videos\\ClippiBoy",
  saveClipHotkey: "Ctrl+Shift+S",
  toggleBufferHotkey: "Ctrl+Shift+B",
  autoStartWithWindows: false,
  onlyBufferInGame: true,
  overlay: {
    enabled: true,
    onClipSaved: true,
    onBufferToggle: true,
    onError: true,
    corner: "bottomRight",
    durationMs: 3500,
    monitor: null,
    followActiveScreen: false,
  },
  trayHintShown: false,
};

export const mockClips: Clip[] = [
  {
    id: "c1",
    path: "C:\\Users\\fabia\\Videos\\ClippiBoy\\cs2_2026-08-17_22-14-03.mp4",
    createdAt: Date.now() - 1000 * 60 * 12,
    durationMs: 32_000,
    game: "Counter-Strike 2",
    width: 1920,
    height: 1080,
    sizeBytes: 168_000_000,
    thumbPath: null,
    title: "Ace auf Mirage",
    description: "Letzte Runde, 1v3 nach dem Retake.",
    edit: null,
    original: null,
  },
  {
    id: "c2",
    path: "C:\\Users\\fabia\\Videos\\ClippiBoy\\cs2_2026-08-17_21-48-51.mp4",
    createdAt: Date.now() - 1000 * 60 * 60 * 3,
    durationMs: 60_000,
    game: "Counter-Strike 2",
    width: 1920,
    height: 1080,
    sizeBytes: 305_000_000,
    thumbPath: null,
    title: null,
    description: null,
    edit: null,
    original: null,
  },
  {
    id: "c3",
    path: "C:\\Users\\fabia\\Videos\\ClippiBoy\\nohesi_2026-08-16_02-11-20.mp4",
    createdAt: Date.now() - 1000 * 60 * 60 * 26,
    durationMs: 18_500,
    game: "No Hesi",
    width: 1920,
    height: 1080,
    sizeBytes: 94_000_000,
    thumbPath: null,
    title: null,
    description: null,
    edit: null,
    original: null,
  },
  // Die Galerie muss auch den schlechten Fall aushalten: Ein Clip ohne Spiel
  // und drei Fehlerkennungen aus der Zeit vor der schärferen Titelprüfung —
  // daran zeigt sich, ob die Filterleiste kürzt, scrollt und aufräumbar ist.
  {
    id: "c4",
    path: "C:\\Users\\fabia\\Videos\\ClippiBoy\\clip_2026-08-15_19-02-44.mp4",
    createdAt: Date.now() - 1000 * 60 * 60 * 30,
    durationMs: 24_000,
    game: null,
    width: 2560,
    height: 1440,
    sizeBytes: 142_000_000,
    thumbPath: null,
    title: null,
    description: null,
    edit: null,
    original: null,
  },
  ...[
    "(102) WIR MÜSSEN PAYEN - YouTube – Opera",
    "C:\\Users\\fabia\\projects\\clippiboy\\synctest.mp4",
    "Snipping Tool Überlagerung",
  ].map((game, i) => ({
    id: `c${5 + i}`,
    path: `C:\\Users\\fabia\\Videos\\ClippiBoy\\clip_2026-08-1${i}_08-30-00.mp4`,
    createdAt: Date.now() - 1000 * 60 * 60 * (40 + i * 5),
    durationMs: 12_000 + i * 3000,
    game,
    width: 1920,
    height: 1080,
    sizeBytes: 60_000_000,
    thumbPath: null,
    title: null,
    description: null,
    edit: null,
    original: null,
  })),
];

/** Ohne Backend gibt es keine echten Tonspuren — der Mixer soll trotzdem da sein. */
export const mockTracks: ClipTrack[] = [
  { index: 0, label: "Hauptmix", channels: 2, previewPath: "mock/0.m4a" },
  { index: 1, label: "Discord", channels: 2, previewPath: "mock/1.m4a" },
  { index: 2, label: "Mikrofon", channels: 2, previewPath: "mock/2.m4a" },
];
