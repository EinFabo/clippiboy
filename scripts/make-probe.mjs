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
const RESOURCES = join(ROOT, "dist-ffmpeg");
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

const README = `ClippiBoy — encoder test
========================

Thanks for helping out. The test measures how your graphics card's video
encoder behaves under different settings. To do that, it records a few
seconds of your screen several times.

IMPORTANT — close these first:

  * Games
  * OBS, ShadowPlay, ClippiBoy, any other recording or streaming software
  * anything else that keeps the graphics card busy

This is not a formality. If any of it keeps running, it shares the encoder
with the test and the measurement is worthless — on the first attempt here,
OBS was running in the background and made a healthy card look broken. The
test now notices this itself and says so in the report, but then the time
was wasted.

How it works:

  1. Unzip this folder anywhere (the desktop is fine).
  2. Double-click clippiboy-probe.exe.
  3. The test checks by itself whether anything is still running and then
     tells you what to close. Close it, press Enter — then it starts.
  4. Keep a video playing while the test measures — any YouTube clip in a
     window will do, as long as something moves. On a still screen the
     encoder has almost nothing to do, and the test measures the wrong thing.
  5. Wait until "Done" appears in the window — about four minutes. The test
     is running the whole time, even when nothing seems to happen. Please
     don't start anything else meanwhile.
  6. Send back the file report.txt that will then be in this folder.
     After that, press Enter and the window closes.

If Windows warns that the publisher is unknown: "More info" and then
"Run anyway".

What gets recorded: the screen content during those seconds. The picture never
leaves your computer — only report.txt is needed, and it contains no picture,
just text: which graphics card, which encoder, how many frames per second. If
you like, take a look inside first; it is a plain text file.

If you have anything confidential open right now, close it first.
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
  await writeFile(join(STAGE, "README.txt"), README, "utf8");

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
