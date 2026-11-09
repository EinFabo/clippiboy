// Reading a ZIP archive with nothing but Node's own zlib — for the fetch
// scripts (`fetch-ffmpeg.mjs`, `fetch-sdl.mjs`). An extra package just for
// unpacking would be more ballast in a build chain that is already large
// enough.

import { inflateRawSync } from "node:zlib";

/** Read the central directory of a ZIP archive. */
export function entries(buf) {
  // Search for the end-of-central-directory from the back — it says where the
  // directory begins.
  let eocd = buf.length - 22;
  while (eocd >= 0 && buf.readUInt32LE(eocd) !== 0x06054b50) eocd--;
  if (eocd < 0) throw new Error("not a ZIP archive: central directory missing");

  const count = buf.readUInt16LE(eocd + 10);
  let at = buf.readUInt32LE(eocd + 16);
  const found = [];
  for (let i = 0; i < count; i++) {
    if (buf.readUInt32LE(at) !== 0x02014b50) throw new Error("ZIP entry damaged");
    const method = buf.readUInt16LE(at + 10);
    const size = buf.readUInt32LE(at + 24);
    const nameLen = buf.readUInt16LE(at + 28);
    const extraLen = buf.readUInt16LE(at + 30);
    const commentLen = buf.readUInt16LE(at + 32);
    const offset = buf.readUInt32LE(at + 42);
    const name = buf.toString("utf8", at + 46, at + 46 + nameLen);
    found.push({ name, method, size, offset });
    at += 46 + nameLen + extraLen + commentLen;
  }
  return found;
}

/** Unpack one file from the archive. */
export function extract(buf, entry) {
  // The local header repeats the field lengths; only it says where the data
  // really begins.
  const head = entry.offset;
  if (buf.readUInt32LE(head) !== 0x04034b50) throw new Error(`no local file header at ${entry.name}`);
  const start = head + 30 + buf.readUInt16LE(head + 26) + buf.readUInt16LE(head + 28);
  const raw = buf.subarray(start, start + entry.size);
  if (entry.method === 0) return Buffer.from(raw);
  if (entry.method === 8) return inflateRawSync(raw);
  throw new Error(`unknown compression (${entry.method}) at ${entry.name}`);
}
