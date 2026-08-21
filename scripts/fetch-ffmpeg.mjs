// Fetches ffmpeg.exe and ffprobe.exe into `src-tauri/resources/`.
//
// Both programs go into the installer package so ClippiBoy can write clips on
// someone else's machine without ffmpeg being on their PATH. Runs before
// `tauri build` (see `npm run app:build`) and in CI.
//
// Deliberately dependency-free: `zlib` gives Node everything needed to read a
// ZIP archive, and an extra package just for unpacking would be more ballast in
// a build chain that is already large enough.

import { createHash } from "node:crypto";
import { inflateRawSync } from "node:zlib";
import { mkdir, readFile, writeFile, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = join(ROOT, "src-tauri", "resources");
const CACHE = join(ROOT, "node_modules", ".cache", "ffmpeg-release-essentials.zip");
const URL_ZIP = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip";
const WANTED = ["ffmpeg.exe", "ffprobe.exe"];

/** Zentrale Verzeichnisliste eines ZIP-Archivs lesen. */
function entries(buf) {
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
function extract(buf, entry) {
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

async function exists(path) {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

async function main() {
  const force = process.argv.includes("--force");
  if (!force && (await Promise.all(WANTED.map((n) => exists(join(OUT, n))))).every(Boolean)) {
    console.log("ffmpeg is already in src-tauri/resources — nothing to do (--force overrides).");
    return;
  }

  await mkdir(dirname(CACHE), { recursive: true });
  let zip;
  if (await exists(CACHE)) {
    console.log(`archive from cache: ${CACHE}`);
    zip = await readFile(CACHE);
  } else {
    console.log(`downloading ${URL_ZIP} …`);
    const response = await fetch(URL_ZIP);
    if (!response.ok) throw new Error(`download failed: HTTP ${response.status}`);
    zip = Buffer.from(await response.arrayBuffer());
    await writeFile(CACHE, zip);
  }
  console.log(`archive: ${(zip.length / 1e6).toFixed(1)} MB, sha256 ${createHash("sha256").update(zip).digest("hex").slice(0, 16)}…`);

  await mkdir(OUT, { recursive: true });
  const list = entries(zip);
  for (const name of WANTED) {
    // The path inside the archive carries the version number, hence going by the file name.
    const entry = list.find((e) => e.name.endsWith(`/bin/${name}`));
    if (!entry) throw new Error(`${name} is not in the archive`);
    const data = extract(zip, entry);
    const target = join(OUT, name);
    await writeFile(target, data);
    console.log(`${name}: ${(data.length / 1e6).toFixed(1)} MB → ${target}`);
  }
}

main().catch((err) => {
  console.error(`could not provide ffmpeg: ${err.message}`);
  process.exit(1);
});
