// The share page at clippiboy.com/c/<id>, in the app's own look: the same
// tokens as src/styles/tokens.css and site/style.css, the white round play
// button and violet scrubber of the app's player (ui/PlayerControls.tsx), and
// while the video loads, the logo with the arc running round it — the
// export's busy ring (ui/BusyVeil.tsx).
//
// Everything is inline: one request for the page, then only the video. The
// font comes from the website, which sits on the same domain.

import { escapeHtml } from "./util";

export interface PageData {
  id: string;
  page: string;
  media: string;
  poster: string;
  title: string;
  game: string | null;
  tags: string[];
  /** Only when the uploader allows it (Settings → Friends). */
  uploader: { name: string; avatar: string | null } | null;
  expires: string;
  width: number;
  height: number;
  site: string;
}

/** The logo's paths, from src/components/ui/Logo.tsx. */
const C = "M177.03 86.86 A64 64 0 1 0 177.03 169.14";
const ARC = "M177.03 169.14 A64 64 0 1 1 177.03 86.86";
const PLAY = "M111.6 102.1 L143.2 128 L111.6 153.9 Z";

/** The logo, optionally with the arc of the busy ring over a faded C. */
function logo(className: string, busy: boolean): string {
  return `<svg class="${className}" viewBox="0 0 256 256" aria-hidden="true">
<defs>
<radialGradient id="d-${className}" cx="34%" cy="24%" r="92%"><stop offset="0" stop-color="#1d1533"/><stop offset=".55" stop-color="#110c1f"/><stop offset="1" stop-color="#08060f"/></radialGradient>
<linearGradient id="r-${className}" x1="0" y1="0" x2=".45" y2="1"><stop offset="0" stop-color="#7c4fd8"/><stop offset=".5" stop-color="#452a7f"/><stop offset="1" stop-color="#2a1a4d"/></linearGradient>
<linearGradient id="m-${className}" x1=".08" y1=".05" x2=".92" y2=".95"><stop offset="0" stop-color="#b492ff"/><stop offset=".45" stop-color="#8b5cf6"/><stop offset="1" stop-color="#6d28d9"/></linearGradient>
<linearGradient id="p-${className}" x1=".1" y1="0" x2=".9" y2="1"><stop offset="0" stop-color="#a78bfa"/><stop offset="1" stop-color="#7c3aed"/></linearGradient>
</defs>
<circle cx="128" cy="128" r="125" fill="url(#d-${className})"/>
<circle cx="128" cy="128" r="123.5" fill="none" stroke="url(#r-${className})" stroke-width="3"/>
<path d="${C}" fill="none" stroke="url(#m-${className})" stroke-width="28" stroke-linecap="round" opacity="${busy ? 0.26 : 1}"/>
${busy ? `<path class="arc" d="${ARC}" pathLength="100" fill="none" stroke="url(#m-${className})" stroke-width="28" stroke-linecap="round"/>` : ""}
<path d="${PLAY}" fill="url(#p-${className})" stroke="url(#p-${className})" stroke-width="9" stroke-linejoin="round"/>
</svg>`;
}

/** What the clip page and the expired page share: tokens, header, buttons. */
function baseCss(site: string): string {
  return `@font-face{font-family:"Inter";src:url("${site}/fonts/inter.woff2") format("woff2");font-weight:100 900;font-display:swap}
:root{color-scheme:dark;--base:#08080a;--surface:#121215;--elevated:#1a1a1f;--hover:#22222a;--line:#26262c;--ink:#fff;--muted:#8a8a96;--faint:#5c5c68;--accent:#8b5cf6;--bright:#a78bfa;--soft:cubic-bezier(.2,.8,.2,1)}
*{box-sizing:border-box}
body{margin:0;min-height:100vh;background:var(--base);color:var(--ink);font:15px/1.5 "Inter",system-ui,-apple-system,"Segoe UI",sans-serif;-webkit-font-smoothing:antialiased}
.hero{position:absolute;inset:0 0 auto;height:420px;background:linear-gradient(180deg,#7c3aed 0%,#4c1d95 22%,#2a1b6b 48%,#08080a 100%);opacity:.9;pointer-events:none}
main{position:relative;max-width:1100px;margin:0 auto;padding:20px 16px 56px}
header{display:flex;align-items:center;justify-content:space-between;gap:16px;margin-bottom:24px}
.brand{display:flex;align-items:center;gap:10px;color:var(--ink);text-decoration:none;font-weight:650;font-size:17px;letter-spacing:-.01em}
.brand svg{width:32px;height:32px}
.btn{display:inline-flex;align-items:center;gap:8px;background:#fff;color:#000;border-radius:999px;padding:0 20px;height:40px;text-decoration:none;font-weight:600;font-size:14px;white-space:nowrap;transition:background .15s}
.btn:hover{background:rgb(255 255 255/.88)}
.cta{display:flex;flex-wrap:wrap;align-items:center;justify-content:space-between;gap:16px;margin-top:32px;padding:20px 22px;border:1px solid var(--line);border-radius:20px;background:linear-gradient(135deg,rgb(139 92 246/.12),transparent 60%),var(--surface)}
.cta p{margin:0;color:var(--muted);font-size:14px;max-width:560px}.cta strong{color:var(--ink)}
footer{margin-top:22px;font-size:12px;color:var(--faint)}footer a{color:var(--muted)}`;
}

/** The page head both pages start with. */
function head(site: string, title: string): string {
  return `<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>${title}</title>
<meta name="robots" content="noindex">
<meta name="theme-color" content="#8b5cf6">
<link rel="icon" href="${site}/img/logo.svg" type="image/svg+xml">
<link rel="preload" href="${site}/fonts/inter.woff2" as="font" type="font/woff2" crossorigin>`;
}

const CTA_TEXT =
  "<strong>Clipped with ClippiBoy</strong> — it keeps the last minutes of your game, so you can save the best moment after it happened. Free for Windows.";

/** A link that ran out or was deleted: the clip's frame, empty, in the same look. */
export function renderGone(site: string): string {
  return `${head(site, "Clip expired · ClippiBoy")}
<meta property="og:site_name" content="ClippiBoy">
<meta property="og:title" content="This clip has expired">
<meta property="og:description" content="Links from ClippiBoy last five days.">
<style>
${baseCss(site)}
.gone{position:relative;aspect-ratio:16/9;max-height:70vh;margin:0 auto;display:grid;place-items:center;text-align:center;padding:24px;background:radial-gradient(ellipse at 50% 40%,rgb(139 92 246/.14),transparent 65%),var(--surface);border:1px solid var(--line);border-radius:20px;box-shadow:0 30px 80px -20px rgb(0 0 0/.7)}
.gone svg{width:84px;height:84px;opacity:.55;filter:grayscale(.35)}
.gone h1{font-size:24px;font-weight:650;letter-spacing:-.02em;margin:18px 0 6px}
.gone p{margin:0;color:var(--muted);font-size:14px;max-width:420px}
@media (max-width:560px){.gone{aspect-ratio:auto;padding:48px 20px;border-radius:14px}.gone h1{font-size:20px}}
</style></head><body><div class="hero"></div><main>
<header><a class="brand" href="${site}">${logo("mark", false)}ClippiBoy</a><a class="btn" href="${site}">Get ClippiBoy</a></header>
<div class="gone"><div>${logo("faded", false)}<h1>This clip has expired</h1><p>Links from ClippiBoy last five days, or less if the clip was deleted. Ask whoever sent it for a new one.</p></div></div>
<div class="cta"><p>${CTA_TEXT}</p><a class="btn" href="${site}">Get ClippiBoy</a></div>
</main></body></html>`;
}

export function renderPage(d: PageData): string {
  const title = escapeHtml(d.title);
  const by = d.uploader ? ` by ${d.uploader.name}` : "";
  const description = `${d.game ? `${d.game} · ` : ""}Shared${by} with ClippiBoy · expires ${d.expires}`;
  const ratio = d.width && d.height ? `${d.width} / ${d.height}` : "16 / 9";
  const pills = [
    d.game ? `<span class="pill game">${escapeHtml(d.game)}</span>` : "",
    ...d.tags.map((tag) => `<span class="pill">${escapeHtml(tag)}</span>`),
  ].join("");
  const uploader = d.uploader
    ? `<div class="by">${
        d.uploader.avatar
          ? `<img src="${escapeHtml(d.uploader.avatar)}" alt="" width="28" height="28" referrerpolicy="no-referrer">`
          : `<span class="initial">${escapeHtml(d.uploader.name.slice(0, 1).toUpperCase())}</span>`
      }<span>Shared by <strong>${escapeHtml(d.uploader.name)}</strong></span></div>`
    : "";

  return `${head(d.site, `${title} · ClippiBoy`)}
<meta property="og:type" content="video.other">
<meta property="og:site_name" content="ClippiBoy">
<meta property="og:title" content="${title}">
<meta property="og:description" content="${escapeHtml(description)}">
<meta property="og:url" content="${d.page}">
<meta property="og:image" content="${d.poster}">
<meta property="og:video" content="${d.media}">
<meta property="og:video:secure_url" content="${d.media}">
<meta property="og:video:type" content="video/mp4">
${d.width ? `<meta property="og:video:width" content="${d.width}"><meta property="og:video:height" content="${d.height}">` : ""}
<meta name="twitter:card" content="summary_large_image">
<style>
${baseCss(d.site)}
.player{position:relative;aspect-ratio:${ratio};max-height:76vh;margin:0 auto;background:#000;border:1px solid var(--line);border-radius:20px;overflow:hidden;box-shadow:0 30px 80px -20px rgb(0 0 0/.7)}
video{width:100%;height:100%;display:block;object-fit:contain;cursor:pointer}
.veil{position:absolute;inset:0;display:grid;place-items:center;background:rgb(8 8 10/.35);transition:opacity .2s var(--soft)}
.veil.off{opacity:0;pointer-events:none}
.loader{width:76px;height:76px;filter:drop-shadow(0 8px 24px rgb(0 0 0/.6))}
.loader .arc{stroke-dasharray:22 180;animation:run 1.5s linear infinite}
@keyframes run{from{stroke-dashoffset:100}to{stroke-dashoffset:-22}}
.big{width:72px;height:72px;border-radius:999px;background:#fff;color:#000;border:0;display:grid;place-items:center;cursor:pointer;box-shadow:0 10px 40px rgb(0 0 0/.5);transition:transform .15s var(--soft)}
.big:hover{transform:scale(1.06)}.big:active{transform:scale(.95)}
.big svg{width:26px;height:26px;translate:2px 0}
.bar{position:absolute;inset:auto 0 0;display:flex;align-items:center;gap:12px;padding:28px 16px 14px;background:linear-gradient(transparent,rgb(0 0 0/.75));transition:opacity .25s var(--soft)}
.player.idle .bar{opacity:0;pointer-events:none}.player.idle{cursor:none}
.play{position:relative;width:36px;height:36px;flex:none;border:0;border-radius:999px;background:#fff;color:#000;display:grid;place-items:center;cursor:pointer;transition:transform .12s}
.play:active{transform:scale(.95)}
.play svg{position:absolute;width:15px;height:15px;transition:opacity .16s var(--soft),transform .16s var(--soft)}
.play .pause{opacity:0;transform:scale(.7) rotate(20deg)}
.playing .play .pause{opacity:1;transform:none}.playing .play .tri{opacity:0;transform:scale(.7) rotate(-20deg)}
.time{font-size:12px;color:#d4d4d8;font-variant-numeric:tabular-nums;white-space:nowrap}
.scrub{position:relative;flex:1;height:28px;cursor:pointer;touch-action:none}
.track{position:absolute;inset:50% 0 auto;height:4px;translate:0 -50%;border-radius:999px;background:rgb(255 255 255/.15);overflow:hidden}
.buffered{position:absolute;inset:0 auto 0 0;background:rgb(255 255 255/.18);border-radius:999px}
.fill{position:absolute;inset:0 auto 0 0;background:var(--bright);border-radius:999px}
.knob{position:absolute;top:50%;width:12px;height:12px;border-radius:999px;background:#fff;translate:-50% -50%;opacity:0;transition:opacity .15s;pointer-events:none}
.scrub:hover .knob,.scrub.drag .knob{opacity:1}
.icon{width:34px;height:34px;flex:none;border:0;border-radius:999px;background:transparent;color:#e4e4e7;display:grid;place-items:center;cursor:pointer;transition:background .15s}
.icon:hover{background:rgb(255 255 255/.12)}.icon svg{width:18px;height:18px}
.info{display:flex;flex-wrap:wrap;align-items:flex-start;justify-content:space-between;gap:16px;margin-top:22px}
h1{font-size:24px;font-weight:650;letter-spacing:-.02em;margin:0;overflow-wrap:anywhere}
.pills{display:flex;flex-wrap:wrap;gap:6px;margin-top:10px}
.pill{border:1px solid var(--line);background:var(--elevated);color:var(--muted);border-radius:999px;padding:3px 11px;font-size:12px;font-weight:500}
.pill.game{color:var(--ink);background:rgb(139 92 246/.18);border-color:rgb(167 139 250/.35)}
.by{display:flex;align-items:center;gap:10px;color:var(--muted);font-size:13px;margin-top:14px}
.by img,.by .initial{width:28px;height:28px;border-radius:999px;background:var(--elevated);object-fit:cover}
.by .initial{display:grid;place-items:center;color:var(--ink);font-weight:600;font-size:12px}
.by strong{color:var(--ink);font-weight:600}
.expires{display:inline-flex;align-items:center;gap:7px;color:var(--muted);font-size:13px;border:1px solid var(--line);background:var(--surface);border-radius:999px;padding:6px 13px;white-space:nowrap}
.expires svg{width:14px;height:14px}
@media (max-width:560px){h1{font-size:20px}.player{border-radius:14px}.time .total{display:none}}
@media (prefers-reduced-motion:reduce){*{transition:none!important}.loader .arc{animation-duration:3s}}
</style></head><body><div class="hero"></div><main>
<header><a class="brand" href="${d.site}">${logo("mark", false)}ClippiBoy</a><a class="btn" href="${d.site}">Get ClippiBoy</a></header>
<div class="player" id="player">
<video id="video" src="${d.media}" poster="${d.poster}" playsinline preload="metadata"></video>
<div class="veil" id="start"><button class="big" id="big" aria-label="Play"><svg viewBox="0 0 24 24" fill="currentColor"><path d="M7.5 5.2 19 12 7.5 18.8V5.2Z"/></svg></button></div>
<div class="veil off" id="loading">${logo("loader", true)}</div>
<div class="bar">
<button class="play" id="play" aria-label="Play"><svg class="tri" viewBox="0 0 24 24" fill="currentColor"><path d="M7.5 5.2 19 12 7.5 18.8V5.2Z"/></svg><svg class="pause" viewBox="0 0 24 24" fill="currentColor"><rect x="6.5" y="5" width="3.6" height="14" rx="1.2"/><rect x="13.9" y="5" width="3.6" height="14" rx="1.2"/></svg></button>
<span class="time"><span id="now">0:00</span><span class="total"> / <span id="len">0:00</span></span></span>
<div class="scrub" id="scrub" role="slider" aria-label="Position" tabindex="0"><div class="track"><div class="buffered" id="buf"></div><div class="fill" id="fill"></div></div><div class="knob" id="knob"></div></div>
<button class="icon" id="mute" aria-label="Mute"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M4 9.5h3l4.5-4v13L7 14.5H4z"/><path id="wave" d="M15.5 9a4 4 0 0 1 0 6M18 6.5a7.5 7.5 0 0 1 0 11"/></svg></button>
<button class="icon" id="full" aria-label="Fullscreen"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/></svg></button>
</div></div>
<div class="info"><div><h1>${title}</h1>${pills ? `<div class="pills">${pills}</div>` : ""}${uploader}</div>
<span class="expires"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/></svg>Expires ${escapeHtml(d.expires)}</span></div>
<div class="cta"><p>${CTA_TEXT}</p><a class="btn" href="${d.site}">Get ClippiBoy</a></div>
<footer>This clip deletes itself ${escapeHtml(d.expires)}. Something wrong with it? <a href="mailto:privacy@clippiboy.com?subject=Clip%20${d.id}">Report it</a>.</footer>
</main>
<script>
(() => {
  const $ = (id) => document.getElementById(id);
  const player = $("player"), video = $("video"), start = $("start"), loading = $("loading");
  const fill = $("fill"), knob = $("knob"), buf = $("buf"), scrub = $("scrub");
  const clock = (s) => { s = Math.floor(s || 0); return Math.floor(s / 60) + ":" + String(s % 60).padStart(2, "0"); };
  const toggle = () => (video.paused ? video.play() : video.pause());
  let started = false, idle;
  const wake = () => { player.classList.remove("idle"); clearTimeout(idle); if (!video.paused) idle = setTimeout(() => player.classList.add("idle"), 2500); };
  const show = (el, on) => el.classList.toggle("off", !on);
  $("big").onclick = () => { started = true; show(start, false); video.play(); };
  video.onclick = () => { if (!started) { started = true; show(start, false); } toggle(); };
  $("play").onclick = toggle;
  video.onplay = () => { player.classList.add("playing"); $("play").ariaLabel = "Pause"; wake(); };
  video.onpause = () => { player.classList.remove("playing"); $("play").ariaLabel = "Play"; wake(); };
  video.onwaiting = () => started && show(loading, true);
  video.onplaying = video.oncanplay = video.onseeked = () => show(loading, false);
  video.onloadedmetadata = () => { $("len").textContent = clock(video.duration); };
  const paint = () => {
    const p = video.duration ? video.currentTime / video.duration : 0;
    fill.style.width = knob.style.left = p * 100 + "%";
    $("now").textContent = clock(video.currentTime);
    if (!video.paused) requestAnimationFrame(paint);
  };
  video.addEventListener("play", () => requestAnimationFrame(paint));
  // While playing, the rAF loop above paints; timeupdate only covers the
  // paused case (a seek), or every update would start another loop.
  video.ontimeupdate = () => { if (video.paused) paint(); };
  video.onprogress = () => { const b = video.buffered; if (b.length && video.duration) buf.style.width = (b.end(b.length - 1) / video.duration) * 100 + "%"; };
  const seekTo = (x) => { const r = scrub.getBoundingClientRect(); if (video.duration) { video.currentTime = Math.min(Math.max((x - r.left) / r.width, 0), 1) * video.duration; paint(); } };
  scrub.onpointerdown = (e) => { scrub.setPointerCapture(e.pointerId); scrub.classList.add("drag"); seekTo(e.clientX); scrub.onpointermove = (m) => seekTo(m.clientX); };
  scrub.onpointerup = () => { scrub.classList.remove("drag"); scrub.onpointermove = null; };
  $("mute").onclick = () => { video.muted = !video.muted; $("wave").style.opacity = video.muted ? 0 : 1; };
  $("full").onclick = () => (document.fullscreenElement ? document.exitFullscreen() : player.requestFullscreen());
  player.onpointermove = wake;
  document.onkeydown = (e) => {
    if (e.target.closest && e.target.closest("a,button")) return;
    if (e.code === "Space" || e.key === "k") { e.preventDefault(); if (!started) { started = true; show(start, false); } toggle(); }
    else if (e.key === "ArrowRight") video.currentTime += 5;
    else if (e.key === "ArrowLeft") video.currentTime -= 5;
    else if (e.key === "f") $("full").onclick();
    else if (e.key === "m") $("mute").onclick();
  };
})();
</script></body></html>`;
}
