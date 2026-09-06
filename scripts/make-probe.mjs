// Packs the diagnostic probe into something a stranger can double-click.
//
// The machines where the encoder misbehaves are not the machines we can build
// on. Somebody else has to run the measurement for us, and everything we can
// ask of them is: unpack, start, send the file back. So the package carries its
// own ffmpeg and writes its report next to itself rather than into a temporary
// folder nobody would find.
//
// Runs on Windows only — it builds a Windows executable and leans on PowerShell
// for the ZIP. Dependency-free for the same reason as `fetch-ffmpeg.mjs`: the
// build chain is large enough already.

import { spawnSync } from "node:child_process";
import { copyFile, mkdir, rm, stat, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const TAURI = join(ROOT, "src-tauri");
const RESOURCES = join(TAURI, "resources");
const TARGET = process.env.CARGO_TARGET_DIR ?? join(TAURI, "target");
// Not under `dist/`: that is Vite's output directory, and it gets emptied on
// every `npm run build`. A package that quietly disappears the next time the
// front end is built is worse than no package.
const OUT = join(ROOT, "probe-package");
const STAGE = join(OUT, "clippiboy-probe");
const ZIP = join(OUT, "clippiboy-probe.zip");

// ffmpeg is 200 MB of the package and the measurement does not need it — it
// only muxes the test clip at the end, as a check that the packets really
// decode. Slim is the sensible default for sending: 1 MB goes into a chat
// message, 76 MB goes onto a file host and then into a conversation about file
// hosts. `--full` puts the tools back for a complete end-to-end run.
const SLIM = !process.argv.includes("--full");

const LIESMICH = `ClippiBoy — Encoder-Test
========================

Danke fürs Mitmachen. Der Test misst, wie sich der Video-Encoder deiner
Grafikkarte unter verschiedenen Einstellungen verhält. Er nimmt dazu mehrmals
ein paar Sekunden deinen Bildschirm auf.

WICHTIG — vorher schließen:

  * Spiele
  * OBS, Shadowplay, ClippiBoy, jede andere Aufnahme- oder Streaming-Software
  * alles, was die Grafikkarte beschäftigt

Das ist keine Förmlichkeit. Läuft etwas davon mit, teilt es sich den Encoder
mit dem Test, und die Messung ist wertlos — beim ersten Versuch hier lief OBS
im Hintergrund und hat eine gesunde Karte kaputt aussehen lassen. Der Test
merkt das inzwischen selbst und sagt es im Bericht, aber dann war die Zeit
umsonst.

So geht es:

  1. Diesen Ordner irgendwohin entpacken (Desktop reicht).
  2. clippiboy-probe.exe doppelklicken.
  3. Der Test schaut selbst nach, ob noch etwas läuft, und sagt dir dann,
     was du schließen sollst. Schließen, Enter drücken — dann geht es los.
  4. Lass ein Video laufen, während der Test misst — irgendein YouTube-Clip
     im Fenster reicht, Hauptsache es bewegt sich etwas. Auf einem stillen
     Bildschirm hat der Encoder fast nichts zu tun, und dann misst der Test
     das Falsche.
  5. Warten, bis "Done" im Fenster steht — etwa vier Minuten. Solange läuft
     der Test, auch wenn zwischendurch nichts passiert. Bitte in der Zeit
     sonst nichts starten.
  6. Die Datei report.txt zurückschicken, die dann in diesem Ordner liegt.
     Danach Enter drücken, das Fenster schließt sich.

Falls Windows warnt, dass der Herausgeber unbekannt ist: "Weitere
Informationen" und dann "Trotzdem ausführen".

Was aufgezeichnet wird: der Bildschirminhalt der genannten Sekunden. Das Bild
verlässt deinen Rechner nicht — gebraucht wird nur die report.txt, und darin
steht kein Bild, sondern nur Text: welche Grafikkarte, welcher Encoder, wie
viele Bilder pro Sekunde. Wenn du magst, schau vorher hinein, es ist eine
normale Textdatei.

Falls du gerade etwas Vertrauliches offen hast, mach es vorher zu.
`;

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { stdio: "inherit", shell: false, ...options });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} ended with ${result.status}`);
}

async function main() {
  if (process.platform !== "win32") {
    throw new Error("this only runs on Windows — the probe is a Windows executable");
  }
  if (!SLIM) {
    for (const tool of ["ffmpeg.exe", "ffprobe.exe"]) {
      if (!existsSync(join(RESOURCES, tool))) {
        throw new Error(`${tool} is missing — run \`npm run ffmpeg\` first`);
      }
    }
  }

  console.log("building the probe…");
  run("cargo", ["build", "--release", "--example", "record-probe"], { cwd: TAURI });

  const built = join(TARGET, "release", "examples", "record-probe.exe");
  if (!existsSync(built)) throw new Error(`cargo built nothing at ${built}`);

  await rm(STAGE, { recursive: true, force: true });
  await mkdir(STAGE, { recursive: true });


  // Renamed on the way in: "record-probe" says what it does to us, and
  // "clippiboy-probe" says who it belongs to on somebody else's desktop.
  await copyFile(built, join(STAGE, "clippiboy-probe.exe"));
  if (!SLIM) {
    for (const tool of ["ffmpeg.exe", "ffprobe.exe"]) {
      await copyFile(join(RESOURCES, tool), join(STAGE, tool));
    }
  }
  await writeFile(join(STAGE, "LIESMICH.txt"), LIESMICH, "utf8");

  await rm(ZIP, { force: true });
  run("powershell.exe", [
    "-NoProfile",
    "-Command",
    `Compress-Archive -Path '${STAGE}\\*' -DestinationPath '${ZIP}'`,
  ]);

  const { size } = await stat(ZIP);
  console.log(`\nready: ${ZIP} (${(size / 1024 / 1024).toFixed(1)} MB${SLIM ? ", without ffmpeg" : ""})`);
  console.log("send that one file; ask for report.txt back.");
  if (SLIM) console.log("`npm run probe -- --full` adds ffmpeg and the test clip.");
}

main().catch((err) => {
  console.error(String(err.message ?? err));
  process.exit(1);
});
