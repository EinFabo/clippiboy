// Holt ffmpeg.exe und ffprobe.exe nach `src-tauri/resources/`.
//
// Die beiden Programme werden mit ins Installationspaket gelegt, damit ClippiBoy
// auf einem fremden Rechner Clips schreiben kann, ohne dass dort ffmpeg im PATH
// liegt. Läuft vor `tauri build` (siehe `npm run app:build`) und in der CI.
//
// Bewusst ohne Abhängigkeiten: Node bringt mit `zlib` alles mit, was zum Lesen
// eines ZIP-Archivs nötig ist, und ein Extra-Paket nur fürs Entpacken wäre in
// einer Build-Kette, die ohnehin schon groß ist, nur weiterer Ballast.

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
  // End-of-central-directory von hinten suchen — davor steht, wo das
  // Verzeichnis anfängt.
  let eocd = buf.length - 22;
  while (eocd >= 0 && buf.readUInt32LE(eocd) !== 0x06054b50) eocd--;
  if (eocd < 0) throw new Error("Kein ZIP-Archiv: Zentralverzeichnis fehlt");

  const count = buf.readUInt16LE(eocd + 10);
  let at = buf.readUInt32LE(eocd + 16);
  const found = [];
  for (let i = 0; i < count; i++) {
    if (buf.readUInt32LE(at) !== 0x02014b50) throw new Error("ZIP-Eintrag beschädigt");
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

/** Eine Datei aus dem Archiv auspacken. */
function extract(buf, entry) {
  // Der lokale Kopf wiederholt die Feldlängen; nur er sagt, wo die Daten
  // wirklich beginnen.
  const head = entry.offset;
  if (buf.readUInt32LE(head) !== 0x04034b50) throw new Error(`Kein Dateikopf bei ${entry.name}`);
  const start = head + 30 + buf.readUInt16LE(head + 26) + buf.readUInt16LE(head + 28);
  const raw = buf.subarray(start, start + entry.size);
  if (entry.method === 0) return Buffer.from(raw);
  if (entry.method === 8) return inflateRawSync(raw);
  throw new Error(`Unbekannte Kompression (${entry.method}) bei ${entry.name}`);
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
    console.log("ffmpeg liegt schon in src-tauri/resources — nichts zu tun (--force erzwingt).");
    return;
  }

  await mkdir(dirname(CACHE), { recursive: true });
  let zip;
  if (await exists(CACHE)) {
    console.log(`Archiv aus dem Cache: ${CACHE}`);
    zip = await readFile(CACHE);
  } else {
    console.log(`Lade ${URL_ZIP} …`);
    const response = await fetch(URL_ZIP);
    if (!response.ok) throw new Error(`Download fehlgeschlagen: HTTP ${response.status}`);
    zip = Buffer.from(await response.arrayBuffer());
    await writeFile(CACHE, zip);
  }
  console.log(`Archiv: ${(zip.length / 1e6).toFixed(1)} MB, sha256 ${createHash("sha256").update(zip).digest("hex").slice(0, 16)}…`);

  await mkdir(OUT, { recursive: true });
  const list = entries(zip);
  for (const name of WANTED) {
    // Der Pfad im Archiv enthält die Versionsnummer, deshalb über den Dateinamen.
    const entry = list.find((e) => e.name.endsWith(`/bin/${name}`));
    if (!entry) throw new Error(`${name} steckt nicht im Archiv`);
    const data = extract(zip, entry);
    const target = join(OUT, name);
    await writeFile(target, data);
    console.log(`${name}: ${(data.length / 1e6).toFixed(1)} MB → ${target}`);
  }
}

main().catch((err) => {
  console.error(`ffmpeg konnte nicht bereitgestellt werden: ${err.message}`);
  process.exit(1);
});
