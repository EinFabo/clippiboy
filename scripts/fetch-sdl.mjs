// Fetches SDL3.dll into `dist-sdl/` — the controllers' reader (`src-tauri/src/sdl.rs`).
//
// The official Windows x64 runtime from libsdl-org's GitHub release, pinned to
// one version and the archive's checksum. The release build puts it beside
// clippiboy.exe (`src-tauri/tauri.release.conf.json`); a dev build finds it
// here by itself. Without it the app runs as before, only without
// controllers — so `cargo test` and CI do not need it.
//
// SDL is under the zlib licence; its LICENSE.txt travels along.
//
// For a new SDL: raise VERSION, put the archive's SHA-256 from the release
// page (asset digest) into SHA256_ZIP, run `npm run sdl -- --force`.

import { createHash } from "node:crypto";
import { mkdir, writeFile, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { entries, extract } from "./zip.mjs";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const VERSION = "3.4.20";
const OUT = join(ROOT, "dist-sdl");
const URL_ZIP = `https://github.com/libsdl-org/SDL/releases/download/release-${VERSION}/SDL3-${VERSION}-win32-x64.zip`;
/** The release asset's digest on GitHub. */
const SHA256_ZIP = "368bed8b24e44ad9ea372990814f2343fcfb9e468abb7f8f4a3bc146c7304752";
/** In the archive → in dist-sdl. */
const WANTED = [
  ["SDL3.dll", "SDL3.dll"],
  ["LICENSE.txt", "SDL3-LICENSE.txt"],
];
const TRIES = 4;

async function exists(path) {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

/** The archive, checked: a mirror's error page or a cut-off download never
    gets unpacked. 1 MB — `fetch` is fine here. */
async function archive() {
  let last;
  for (let attempt = 1; attempt <= TRIES; attempt++) {
    try {
      const response = await fetch(URL_ZIP, { headers: { "user-agent": "clippiboy-build" } });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const zip = Buffer.from(await response.arrayBuffer());
      const hash = createHash("sha256").update(zip).digest("hex");
      if (hash !== SHA256_ZIP) throw new Error(`archive checksum ${hash} is not the pinned one`);
      return zip;
    } catch (err) {
      last = err;
      console.warn(`attempt ${attempt}/${TRIES} failed: ${err.message}`);
      if (attempt < TRIES) await new Promise((wait) => setTimeout(wait, attempt * 5000));
    }
  }
  throw last;
}

async function main() {
  const force = process.argv.includes("--force");
  if (!force && (await Promise.all(WANTED.map(([, to]) => exists(join(OUT, to))))).every(Boolean)) {
    console.log("SDL3 is already in dist-sdl — nothing to do (--force overrides).");
    return;
  }
  console.log(`downloading ${URL_ZIP} …`);
  const zip = await archive();
  const list = entries(zip);
  await mkdir(OUT, { recursive: true });
  for (const [from, to] of WANTED) {
    const entry = list.find((e) => e.name === from);
    if (!entry) throw new Error(`${from} is not in the archive`);
    const target = join(OUT, to);
    await writeFile(target, extract(zip, entry));
    console.log(`${from} → ${target}`);
  }
}

main().catch((err) => {
  console.error(`could not provide SDL3: ${err.message}`);
  process.exit(1);
});
