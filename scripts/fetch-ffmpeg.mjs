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
import { get } from "node:https";
import { inflateRawSync } from "node:zlib";
import { mkdir, readFile, rename, writeFile, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = join(ROOT, "src-tauri", "resources");
const CACHE = join(ROOT, "node_modules", ".cache", "ffmpeg-release-essentials.zip");
const URL_ZIP = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip";
const WANTED = ["ffmpeg.exe", "ffprobe.exe"];
const TRIES = 4;

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

/**
 * Locate the two programs in the archive.
 *
 * Doubles as the test of whether a buffer is an archive at all, and that is the
 * point: a mirror can answer an error page with status 200, a transfer can
 * break off without anything looking wrong, and a build can be interrupted
 * mid-write. None of that may reach the cache — the next run would take it for
 * a good archive and ship a broken ffmpeg. Truncation in particular cannot
 * survive this: the central directory sits at the very end of a ZIP.
 */
function programs(buf) {
  const list = entries(buf);
  return WANTED.map((name) => {
    // The path inside the archive carries the version number, hence going by the file name.
    const entry = list.find((e) => e.name.endsWith(`/bin/${name}`));
    if (!entry) throw new Error(`${name} is not in the archive`);
    return { name, entry };
  });
}

/**
 * Fetch the archive.
 *
 * Deliberately `https.get` rather than `fetch`: the latter gives up on a body
 * that takes longer than five minutes, and a hundred megabytes from a single
 * slow mirror does exactly that on a build machine. There is no way to raise
 * that limit without pulling in undici, which this file is not going to do.
 */
function download(url, depth = 0) {
  return new Promise((resolve, reject) => {
    if (depth > 5) {
      reject(new Error("too many redirects"));
      return;
    }
    const request = get(url, { headers: { "user-agent": "clippiboy-build" } }, (response) => {
      const { statusCode, headers } = response;
      if (statusCode >= 300 && statusCode < 400 && headers.location) {
        response.resume();
        resolve(download(new URL(headers.location, url).toString(), depth + 1));
        return;
      }
      if (statusCode !== 200) {
        response.resume();
        reject(new Error(`HTTP ${statusCode}`));
        return;
      }
      const expected = Number(headers["content-length"]);
      const chunks = [];
      response.on("data", (chunk) => chunks.push(chunk));
      response.on("end", () => {
        const body = Buffer.concat(chunks);
        // A connection that dies mid-body does not always look like an error.
        // Where the header is there, it names the damage plainly and early;
        // where it is not, `programs()` is what stands between a half archive
        // and the cache. Hence a note rather than a refusal — a chunked mirror
        // is not a reason to fail a release build.
        if (expected > 0 && body.length !== expected) {
          reject(new Error(`incomplete: ${body.length} of ${expected} bytes`));
          return;
        }
        if (!(expected > 0)) console.warn("no content-length — completeness is decided when unpacking");
        resolve(body);
      });
      response.on("error", reject);
    });
    request.on("error", reject);
    // Only against a connection that stops sending altogether — a slow one is
    // allowed to take its time.
    request.setTimeout(120_000, () => request.destroy(new Error("no data for two minutes")));
  });
}

/**
 * The same, but it does not give up on the first bad day.
 *
 * The mirror is a single host with no CDN behind it, and it throttles and drops
 * connections when several builds ask at once — which is exactly when a release
 * is being cut.
 */
async function downloadWithRetries(url) {
  let last;
  for (let attempt = 1; attempt <= TRIES; attempt++) {
    try {
      return await download(url);
    } catch (err) {
      last = err;
      console.warn(`attempt ${attempt}/${TRIES} failed: ${err.message}`);
      if (attempt < TRIES) {
        await new Promise((wait) => setTimeout(wait, attempt * 5000));
      }
    }
  }
  throw last;
}

/**
 * The archive — from the cache when that still holds a whole one.
 *
 * Nothing is cached before it has proven readable, and nothing already cached
 * is believed without asking again: otherwise one bad answer from the mirror
 * would settle the matter for good, and no number of retries could ever get
 * past it because the download would never run again.
 */
async function archive(force) {
  if (!force && (await exists(CACHE))) {
    const cached = await readFile(CACHE);
    try {
      programs(cached);
      console.log(`archive from cache: ${CACHE}`);
      return cached;
    } catch (err) {
      console.warn(`cached archive unusable (${err.message}) — fetching it again`);
    }
  }

  console.log(`downloading ${URL_ZIP} …`);
  const zip = await downloadWithRetries(URL_ZIP);
  programs(zip);
  // A whole file or none at all: a `.part` an interrupted build leaves behind
  // is never read again, a half-written CACHE would be.
  const part = `${CACHE}.part`;
  await writeFile(part, zip);
  await rename(part, CACHE);
  return zip;
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
  const zip = await archive(force);
  console.log(`archive: ${(zip.length / 1e6).toFixed(1)} MB, sha256 ${createHash("sha256").update(zip).digest("hex").slice(0, 16)}…`);

  await mkdir(OUT, { recursive: true });
  for (const { name, entry } of programs(zip)) {
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
