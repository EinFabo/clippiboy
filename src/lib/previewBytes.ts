/**
 * A binary answer from the core as bytes: an ArrayBuffer over Tauri's IPC
 * channel, a list of numbers over its fallback (taken when the IPC channel is
 * blocked, as the release's CSP once did).
 */
export function previewBytes(raw: ArrayBuffer | number[]): ArrayBuffer {
  return raw instanceof ArrayBuffer ? raw : new Uint8Array(raw).buffer;
}
