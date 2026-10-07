// Renders ClippiBoy's notification sounds into src-tauri/assets/sounds.
//
// The sounds were chosen in the sound lab (plan item 112, round 2): every one is
// synthesised with Web Audio, so this file carries the synthesis itself and
// renders it offline with the same code the lab played.
//
//   npm i --no-save node-web-audio-api
//   node scripts/render-sounds.cjs
//
// Change PICKS or TUNING and run it again to replace the WAVs.
const fs = require("fs");
const path = require("path");
const { OfflineAudioContext } = require("node-web-audio-api");

const OUT = path.join(__dirname, "..", "src-tauri", "assets", "sounds");
/** Event → sound family from the lab (g04 Marimba, g06 Soft-Chip, g07 Tap). */
const PICKS = { clip: "g06", shot: "g07", recStart: "g06", recStop: "g06", bufOn: "g06", bufOff: "g06", friend: "g06", received: "g06", error: "g04" };
/** The lab's sliders as Fabi left them: room %, warmth, semitones, length %. */
const TUNING = { room: 5, warm: 22, pitch: -4, len: 60 };
const FILES = {
  clip: "clip-saved.wav", shot: "screenshot.wav", recStart: "recording-started.wav", recStop: "recording-saved.wav",
  bufOn: "buffer-on.wav", bufOff: "buffer-off.wav", friend: "friend-online.wav", received: "clip-received.wav", error: "error.wav",
};

// ---- the lab's synthesis, verbatim ----
const SYNTH = String.raw`
let ctx = null, voiceIn, lowpass, wet, dry, master, bed = null, bedGain = null;
const T = { room: 18, warm: 45, pitch: 0, len: 100 };
const DEFAULT_T = { ...T };
const m = (n) => 440 * Math.pow(2, (n + T.pitch - 69) / 12);
const L = (s) => s * T.len / 100;
const warmHz = () => 12000 * Math.pow(2600 / 12000, T.warm / 100); // 0 → 12 kHz, 100 → 2,6 kHz

function impulse(seconds = 1.3, decay = 3.2) {
  const rate = ctx.sampleRate, len = Math.floor(rate * seconds), ir = ctx.createBuffer(2, len, rate);
  const pre = Math.floor(rate * 0.012);
  for (let c = 0; c < 2; c++) {
    const d = ir.getChannelData(c); let lp = 0;
    for (let i = pre; i < len; i++) {
      // gefiltertes Rauschen: die Höhen klingen schneller ab als die Mitten — wie ein echter Raum
      const k = 0.25 + 0.6 * (i / len);
      lp += k * ((Math.random() * 2 - 1) - lp);
      d[i] = lp * Math.pow(1 - (i - pre) / (len - pre), decay);
    }
  }
  return ir;
}
function audio() {
  if (!ctx) {
    ctx = makeCtx();
    voiceIn = ctx.createGain();
    const hp = ctx.createBiquadFilter(); hp.type = "highpass"; hp.frequency.value = 170; hp.Q.value = 0.6;
    lowpass = ctx.createBiquadFilter(); lowpass.type = "lowpass"; lowpass.Q.value = 0.5; lowpass.frequency.value = warmHz();
    const sat = ctx.createWaveShaper(); const curve = new Float32Array(1024);
    for (let i = 0; i < 1024; i++) { const x = i / 511.5 - 1; curve[i] = Math.tanh(1.4 * x) / Math.tanh(1.4); }
    sat.curve = curve; sat.oversample = "4x";
    const verb = ctx.createConvolver(); verb.buffer = impulse();
    dry = ctx.createGain(); wet = ctx.createGain();
    master = ctx.createGain(); master.gain.value = vol.value / 100;
    const comp = ctx.createDynamicsCompressor();
    comp.threshold.value = -16; comp.knee.value = 12; comp.ratio.value = 3; comp.attack.value = 0.003; comp.release.value = 0.15;
    voiceIn.connect(hp).connect(lowpass).connect(sat);
    sat.connect(dry).connect(master);
    sat.connect(verb).connect(wet).connect(master);
    master.connect(comp).connect(ctx.destination);
    applyTune();
  }
  
  return ctx;
}
function applyTune() {
  if (!ctx) return;
  wet.gain.value = T.room / 100 * 1.6;
  dry.gain.value = 1 - T.room / 100 * 0.5;
  lowpass.frequency.value = warmHz();
}

// ---------- Bausteine ----------
/** Ein Teilton: Sinus (oder Dreieck), weicher Anstieg, natürlich-exponentielles Abklingen. */
function partial(t, f, { g = 0.2, a = 0.004, d = 0.3, type = "sine", pan = 0, glide = 0 } = {}) {
  const o = ctx.createOscillator(); o.type = type;
  if (glide) { o.frequency.setValueAtTime(f * (1 + glide), t); o.frequency.exponentialRampToValueAtTime(f, t + 0.025); }
  else o.frequency.value = f;
  const v = ctx.createGain(); d = L(d);
  v.gain.setValueAtTime(0, t);
  v.gain.linearRampToValueAtTime(g, t + a);
  v.gain.setTargetAtTime(0, t + a, d / 3.2);
  let node = o.connect(v);
  if (pan && ctx.createStereoPanner) { const p = ctx.createStereoPanner(); p.pan.value = pan; node = node.connect(p); }
  node.connect(voiceIn);
  o.start(t); o.stop(t + a + d * 2.2 + 0.05);
}
let tickBuf = null;
/** Ein winziger Anschlag (≈ 8 ms abklingendes Rauschen, Bandpass) — gibt dem Ton eine Kontur, ohne nach Rauschen zu klingen. */
function tick(t, { f = 4500, q = 2.5, g = 0.12 } = {}) {
  if (!tickBuf) {
    tickBuf = ctx.createBuffer(1, Math.floor(ctx.sampleRate * 0.03), ctx.sampleRate);
    const d = tickBuf.getChannelData(0);
    for (let i = 0; i < d.length; i++) d[i] = (Math.random() * 2 - 1) * Math.exp(-i / (ctx.sampleRate * 0.0018));
  }
  const s = ctx.createBufferSource(); s.buffer = tickBuf;
  const bp = ctx.createBiquadFilter(); bp.type = "bandpass"; bp.frequency.value = f; bp.Q.value = q;
  const v = ctx.createGain(); v.gain.value = g;
  s.connect(bp).connect(v).connect(voiceIn); s.start(t);
}
/** FM-Stimme mit abklingender Modulationstiefe — für Rhodes und Marimba. */
function fm(t, f, { g = 0.2, ratio = 1, index = 1, idxDecay = 0.25, d = 0.6, a = 0.003 } = {}) {
  const c = ctx.createOscillator(), mo = ctx.createOscillator(), mg = ctx.createGain(), v = ctx.createGain();
  c.frequency.value = f; mo.frequency.value = f * ratio; d = L(d);
  mg.gain.setValueAtTime(f * index, t);
  mg.gain.setTargetAtTime(0, t, idxDecay / 3);
  mo.connect(mg).connect(c.frequency);
  v.gain.setValueAtTime(0, t); v.gain.linearRampToValueAtTime(g, t + a); v.gain.setTargetAtTime(0, t + a, d / 3.2);
  c.connect(v).connect(voiceIn);
  c.start(t); mo.start(t); c.stop(t + d * 2.2 + 0.05); mo.stop(t + d * 2.2 + 0.05);
}

// ---------- Instrumente: (Zeit, MIDI-Note, Stärke 0..1) ----------
const INSTR = {
  // Glas: Grundton, Oktave, leiser 3. Teilton; ein zweiter Grundton +4 Cent rechts → Breite statt Schärfe
  glass: (t, n, v = 1) => {
    const f = m(n);
    partial(t, f, { g: 0.2 * v, d: 0.65, pan: -0.25 });
    partial(t, f * 1.0023, { g: 0.12 * v, d: 0.6, pan: 0.3 });
    partial(t, f * 2, { g: 0.06 * v, d: 0.3 });
    partial(t, f * 3, { g: 0.018 * v, d: 0.12 });
    tick(t, { f: 6000, g: 0.05 * v });
  },
  // Kristall: fast reiner Sinus, langer weicher Ausklang
  crystal: (t, n, v = 1) => {
    const f = m(n);
    partial(t, f, { g: 0.24 * v, a: 0.008, d: 0.9, pan: -0.15 });
    partial(t, f * 2.001, { g: 0.035 * v, a: 0.008, d: 0.5, pan: 0.2 });
  },
  // Kalimba: gezupfte Zunge — kurzer Tonhöhen-Rutsch von oben, Oberton bei ~5,9×, kleiner Holzanschlag
  kalimba: (t, n, v = 1) => {
    const f = m(n);
    partial(t, f, { g: 0.24 * v, a: 0.002, d: 0.5, glide: 0.012 });
    partial(t, f * 5.9, { g: 0.03 * v, a: 0.001, d: 0.06 });
    tick(t, { f: 1400, q: 1.2, g: 0.07 * v });
  },
  // Marimba: Tri-Tone-Richtung — Grundton plus kurzer 4. Teilton, weich
  marimba: (t, n, v = 1) => {
    const f = m(n - 12);
    fm(t, f, { g: 0.26 * v, ratio: 4, index: 0.9, idxDecay: 0.05, d: 0.42, a: 0.002 });
    partial(t, f * 2, { g: 0.05 * v, d: 0.2 });
    tick(t, { f: 2200, q: 1.5, g: 0.04 * v });
  },
  // Rhodes: E-Piano — FM 1:1, die Modulation („Biss“) klingt schnell ab, der Körper bleibt warm
  rhodes: (t, n, v = 1) => {
    const f = m(n - 12);
    fm(t, f, { g: 0.22 * v, ratio: 1, index: 1.6, idxDecay: 0.18, d: 0.7 });
    partial(t, f * 2, { g: 0.04 * v, d: 0.35, pan: 0.2 });
    tick(t, { f: 3000, q: 2, g: 0.03 * v });
  },
  // Soft-Chip: 8-Bit-Melodie, aber Dreieck statt Rechteck und kurz gehalten — rund statt kratzig
  chip: (t, n, v = 1) => {
    const f = m(n);
    partial(t, f, { g: 0.22 * v, a: 0.003, d: 0.22, type: "triangle" });
    partial(t, f * 2, { g: 0.04 * v, a: 0.003, d: 0.1, type: "triangle", pan: 0.2 });
  },
  // Tap: runder Klick — winziger Anschlag plus sehr kurzer Sinus-„Tupf“
  tap: (t, n, v = 1) => {
    const f = m(n);
    tick(t, { f: 3800, q: 3, g: 0.14 * v });
    partial(t, f, { g: 0.2 * v, a: 0.002, d: 0.09 });
    partial(t, f / 2, { g: 0.08 * v, a: 0.002, d: 0.06 });
  },
};

// ---------- Motive (für alle Familien gleich) ----------
// D-Dur pentatonisch um D5: 74 D · 76 E · 78 F# · 81 A · 83 H · 86 D' · 88 E' · 90 F#' · 93 A'
const seq = (notes, step, vel = 1) => (play, t) =>
  notes.forEach((n, i) => [].concat(n).forEach((k) => play(t + i * step, k, Array.isArray(vel) ? vel[i] : vel)));
const MOTIF = {
  clip: seq([81, 86], 0.075),
  recStart: seq([74, 78, 81], 0.07),
  recStop: seq([86, 81, 74], 0.08, [0.8, 0.85, 1]),
  bufOn: seq([74, 81], 0.075),
  bufOff: seq([81, 74], 0.085, 0.75),
  friend: seq([88, 93], 0.12, 0.85),
  received: seq([86, 90, 93, 98], 0.05, [0.7, 0.75, 0.8, 0.9]),
  // Fehler: tiefer, ein enger Zweiklang und ein Schritt abwärts — ernst, aber nicht schrill
  error: seq([[69, 70], 66], 0.14, [0.75, 0.9]),
};
// Screenshot: ein Verschluss — zwei runde Anschläge, der zweite tiefer, dazu ein heller Ton
function shutter(play, t) {
  tick(t, { f: 5200, q: 3, g: 0.16 });
  tick(t + 0.055, { f: 3000, q: 3, g: 0.13 });
  play(t + 0.055, 90, 0.55);
}

const EVENTS = [
  ["clip", "Clip", "Clip gespeichert"],
  ["shot", "Screenshot", "Bild gemacht"],
  ["recStart", "Rec ●", "Aufnahme startet"],
  ["recStop", "Rec ■", "Aufnahme gespeichert"],
  ["bufOn", "Buffer an", "Puffer läuft"],
  ["bufOff", "Buffer aus", "Puffer gestoppt"],
  ["friend", "Freund", "kommt online"],
  ["received", "Empfangen", "Clip von Freund"],
  ["error", "Fehler", "etwas ging schief"],
];
const FAMILIES = [
  { id: "g01", name: "Glas", instr: "glass", desc: "Dein Favorit aus Runde 1, sauber gebaut: Grundton und Oktave, leicht verbreitert, kleiner Raum." },
  { id: "g02", name: "Kristall", instr: "crystal", desc: "Fast reiner Sinus mit langem, weichem Ausklang. Die rundeste Variante, sehr unaufdringlich." },
  { id: "g03", name: "Kalimba", instr: "kalimba", desc: "Gezupfte Metallzunge mit einem Hauch Holz. Freundlich und klar, schneidet gut durch Spielton." },
  { id: "g04", name: "Marimba", instr: "marimba", desc: "Weicher Holzschlag in der Art des iPhone-Tri-Tone. Warm und eindeutig." },
  { id: "g05", name: "Rhodes", instr: "rhodes", desc: "E-Piano: kurzer heller Biss, dann ein warmer Körper. Klingt edel und erwachsen." },
  { id: "g06", name: "Soft-Chip", instr: "chip", desc: "Deine Arcade-Picks als runde Fassung: Dreieck statt Rechteck, kurz und mit Raum." },
  { id: "g07", name: "Tap", instr: "tap", desc: "Deine Klick-Picks als runde Fassung: winziger Anschlag mit einem kurzen Ton dahinter." },
];
const playEvent = (fam, ev, t) => {
  const p = INSTR[fam.instr];
  if (ev === "shot") shutter(p, t); else MOTIF[ev](p, t);
};

`;
// ---- end of the lab's code ----

const RATE = 48000, SECONDS = 2.5;

const api = new Function("makeCtx", "vol", SYNTH + `
  return { T, FAMILIES, playEvent, audio, applyTune, reset: () => { ctx = null; tickBuf = null; } };
`);

(async () => {
  for (const [ev, famId] of Object.entries(PICKS)) {
    let off;
    const lab = api(() => (off = new OfflineAudioContext(2, RATE * SECONDS, RATE)), { value: 70 });
    Object.assign(lab.T, TUNING);
    lab.audio();
    lab.applyTune();
    lab.playEvent(lab.FAMILIES.find((f) => f.id === famId), ev, 0.01);
    const buf = await off.startRendering();
    const l = buf.getChannelData(0), r = buf.getChannelData(1);
    // Ende: bis der Pegel dauerhaft unter -70 dB liegt, plus 20 ms Ausblenden
    let top = 0;
    for (let i = 0; i < l.length; i++) top = Math.max(top, Math.abs(l[i]), Math.abs(r[i]));
    // End: where the tail stays 48 dB below the peak — past that it is room nobody hears
    const floor = top * Math.pow(10, -48 / 20);
    let end = l.length;
    while (end > 0 && Math.abs(l[end - 1]) < floor && Math.abs(r[end - 1]) < floor) end--;
    end = Math.min(l.length, end + Math.floor(RATE * 0.02));
    let peak = 0, sum = 0;
    for (let i = 0; i < end; i++) { peak = Math.max(peak, Math.abs(l[i]), Math.abs(r[i])); sum += l[i] * l[i] + r[i] * r[i]; }
    const rms = Math.sqrt(sum / (2 * end));
    // Lautheit angleichen: RMS auf -20 dBFS, Spitze höchstens -1 dBFS
    const gain = Math.min(0.1 / rms, 0.89 / peak);
    const pcm = Buffer.alloc(44 + end * 4);
    pcm.write("RIFF", 0); pcm.writeUInt32LE(36 + end * 4, 4); pcm.write("WAVE", 8);
    pcm.write("fmt ", 12); pcm.writeUInt32LE(16, 16); pcm.writeUInt16LE(1, 20); pcm.writeUInt16LE(2, 22);
    pcm.writeUInt32LE(RATE, 24); pcm.writeUInt32LE(RATE * 4, 28); pcm.writeUInt16LE(4, 32); pcm.writeUInt16LE(16, 34);
    pcm.write("data", 36); pcm.writeUInt32LE(end * 4, 40);
    const fade = Math.floor(RATE * 0.02);
    for (let i = 0; i < end; i++) {
      const f = i > end - fade ? (end - i) / fade : 1;
      pcm.writeInt16LE(Math.round(Math.max(-1, Math.min(1, l[i] * gain * f)) * 32767), 44 + i * 4);
      pcm.writeInt16LE(Math.round(Math.max(-1, Math.min(1, r[i] * gain * f)) * 32767), 46 + i * 4);
    }
    fs.writeFileSync(path.join(OUT, FILES[ev]), pcm);
    console.log(ev, famId, `${(end / RATE * 1000).toFixed(0)} ms`, `peak ${(20 * Math.log10(peak)).toFixed(1)} dB`, `gain ${(20 * Math.log10(gain)).toFixed(1)} dB`);
  }
})();
