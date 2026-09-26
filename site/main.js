"use strict";

const REPO = "EinFabo/clippiboy";
const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;

/* ---------- Deterministic noise, so the same moment always looks the same ---------- */

function hash(n, seed) {
  let x = Math.imul((n | 0) ^ seed, 0x9e3779b1);
  x ^= x >>> 15; x = Math.imul(x, 0x85ebca6b);
  x ^= x >>> 13; x = Math.imul(x, 0xc2b2ae35);
  x ^= x >>> 16;
  return (x >>> 0) / 4294967296;
}
function smooth(i, scale, seed) {
  const p = i / scale, a = Math.floor(p), f = p - a, s = f * f * (3 - 2 * f);
  return hash(a, seed) * (1 - s) + hash(a + 1, seed) * s;
}

// One value 0..1 per column and track.
const tracks = {
  game(i) {
    const fight = smooth(i, 40, 11);
    return Math.min(1, 0.18 + 0.25 * smooth(i, 6, 12) + (fight > 0.6 ? (fight - 0.6) * 1.6 : 0) + 0.2 * hash(i, 13));
  },
  discord(i) {
    const talking = smooth(i, 18, 21) > 0.55;
    return talking ? 0.25 + 0.55 * smooth(i, 2.5, 22) * (0.6 + 0.4 * hash(i, 23)) : 0.03 * hash(i, 24);
  },
  mic(i) {
    const talking = smooth(i, 26, 31) > 0.68;
    return talking ? 0.2 + 0.5 * smooth(i, 2, 32) * (0.6 + 0.4 * hash(i, 33)) : 0.02 * hash(i, 34);
  },
};
const COLORS = { game: "#8b5cf6", discord: "#a78bfa", mic: "#ddd6fe" };

function fitCanvas(canvas) {
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const w = canvas.clientWidth, h = canvas.clientHeight;
  if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
  }
  const ctx = canvas.getContext("2d");
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return { ctx, w, h };
}

/* ---------- The buffer strip ---------- */

(function buffer() {
  const canvas = document.getElementById("buffer");
  const toast = document.getElementById("toast");
  const tryBtn = document.getElementById("try");
  if (!canvas) return;

  const SPAN = 120;        // seconds the strip shows
  const CLIP = 30;         // seconds a save takes
  const COL = 4;           // px per column
  const lanes = [1.3, 1, 1, 1];
  const laneSum = lanes.reduce((a, b) => a + b, 0);

  let t = 0;               // seconds of buffer time
  let last = performance.now();
  let visible = true;
  let saves = [];          // { at, shownAt }
  let toastTimer = 0;

  function save() {
    saves.push({ at: t, shownAt: performance.now() });
    saves = saves.slice(-3);
    showBanner();
    if (reduced) draw();
  }

  // The same card the app puts over the game: in, line around, rim, out.
  function showBanner() {
    clearTimeout(toastTimer);
    toast.hidden = false;
    toast.classList.remove("in", "out");
    void toast.offsetWidth; // restart the animations
    toast.classList.add("in");
    toastTimer = setTimeout(() => {
      toast.classList.replace("in", "out");
      toastTimer = setTimeout(() => { toast.hidden = true; }, 260);
    }, 3500);
  }

  function draw() {
    const { ctx, w, h } = fitCanvas(canvas);
    const speed = w / SPAN;                 // px per second
    const colSec = COL / speed;             // seconds per column
    const now = performance.now();
    ctx.clearRect(0, 0, w, h);

    // lanes
    let y = 0;
    const rows = lanes.map((f) => { const r = { y, h: (h * f) / laneSum }; y += r.h; return r; });

    const first = Math.floor((t - SPAN) / colSec) - 1;
    const lastCol = Math.ceil(t / colSec);
    const xOf = (sec) => w - (t - sec) * speed;

    // picture: tiles, one every six columns
    const pic = rows[0];
    for (let c = Math.floor(first / 6) * 6; c <= lastCol; c += 6) {
      const x = xOf(c * colSec);
      const light = 16 + 30 * smooth(c, 60, 41) + (hash(c, 42) > 0.93 ? 22 : 0);
      const hue = 250 + 40 * smooth(c, 90, 43);
      ctx.fillStyle = `hsl(${hue} 40% ${light}%)`;
      roundRect(ctx, x + 1, pic.y + 12, COL * 6 - 3, pic.h - 20, 4);
    }

    // audio lanes
    ["game", "discord", "mic"].forEach((name, k) => {
      const r = rows[k + 1], mid = r.y + r.h / 2, amp = r.h * 0.4;
      ctx.fillStyle = COLORS[name];
      for (let c = first; c <= lastCol; c++) {
        const v = tracks[name](c);
        const bh = Math.max(1, v * amp * 2);
        ctx.fillRect(xOf(c * colSec), mid - bh / 2, COL - 1.5, bh);
      }
    });

    // lane rules
    ctx.fillStyle = "rgba(255,255,255,0.06)";
    rows.slice(1).forEach((r) => ctx.fillRect(0, r.y, w, 1));

    // saved clips
    for (const s of saves) {
      const age = (now - s.shownAt) / 1000;
      const grow = reduced ? 1 : Math.min(1, age / 0.45);
      const eased = 1 - Math.pow(1 - grow, 3);
      const fade = age > 5 ? Math.max(0, 1 - (age - 5) / 1.2) : 1;
      if (fade <= 0) continue;
      const x1 = xOf(s.at), x0 = xOf(s.at - CLIP * eased);
      ctx.globalAlpha = fade;
      ctx.fillStyle = "rgba(139,92,246,0.2)";
      ctx.fillRect(x0, 0, x1 - x0, h);
      ctx.fillStyle = "#a78bfa";
      ctx.fillRect(x0, 0, 2, h);
      ctx.fillRect(x1 - 2, 0, 2, h);
      ctx.fillRect(x0, 0, 10, 2); ctx.fillRect(x0, h - 2, 10, 2);
      ctx.fillRect(x1 - 10, 0, 10, 2); ctx.fillRect(x1 - 10, h - 2, 10, 2);
      ctx.globalAlpha = 1;
    }
    saves = saves.filter((s) => (now - s.shownAt) / 1000 < 6.2);

    // now
    const g = ctx.createLinearGradient(w - 60, 0, w, 0);
    g.addColorStop(0, "rgba(18,18,21,0)");
    g.addColorStop(1, "rgba(18,18,21,0.9)");
    ctx.fillStyle = g;
    ctx.fillRect(w - 60, 0, 60, h);
    ctx.fillStyle = "#ef4444";
    ctx.fillRect(w - 2, 0, 2, h);
  }

  function roundRect(ctx, x, y, w, h, r) {
    ctx.beginPath();
    ctx.moveTo(x + r, y);
    ctx.arcTo(x + w, y, x + w, y + h, r);
    ctx.arcTo(x + w, y + h, x, y + h, r);
    ctx.arcTo(x, y + h, x, y, r);
    ctx.arcTo(x, y, x + w, y, r);
    ctx.fill();
  }

  function frame(ts) {
    const dt = Math.min(0.1, (ts - last) / 1000);
    last = ts;
    if (visible && !document.hidden) {
      t += dt;
      draw();
    }
    requestAnimationFrame(frame);
  }

  t = 1000; // start somewhere in the middle of an evening
  if (reduced) {
    draw();
    addEventListener("resize", draw);
  } else {
    requestAnimationFrame((ts) => { last = ts; frame(ts); });
    new IntersectionObserver(([e]) => { visible = e.isIntersecting; }).observe(canvas);
    // The one thing the page does by itself: show a save once.
    setTimeout(() => { if (visible && !saves.length) save(); }, 2400);
  }

  tryBtn.addEventListener("click", save);
})();

/* ---------- The mixer ---------- */

(function mixer() {
  const rows = [...document.querySelectorAll(".mix-row[data-track]")];
  const sum = document.getElementById("sum");
  if (!rows.length || !sum) return;

  const N = 120;
  const OFFSET = 1400;

  const gainOf = (row) => {
    const muted = row.querySelector(".mute").getAttribute("aria-pressed") === "true";
    return muted ? 0 : Math.pow(10, Number(row.querySelector("input").value) / 20);
  };

  function bars(canvas, values, color) {
    const { ctx, w, h } = fitCanvas(canvas);
    ctx.clearRect(0, 0, w, h);
    const bw = w / values.length, mid = h / 2;
    values.forEach((v, i) => {
      const clipped = v > 1;
      const bh = Math.max(1, Math.min(1, v) * (h - 2));
      ctx.fillStyle = clipped ? "#ef4444" : color;
      ctx.fillRect(i * bw, mid - bh / 2, Math.max(1, bw - 1.2), bh);
    });
  }

  function render() {
    const total = new Array(N).fill(0);
    rows.forEach((row) => {
      const name = row.dataset.track;
      const g = gainOf(row);
      const vals = [];
      for (let i = 0; i < N; i++) {
        const v = tracks[name](OFFSET + i) * 0.5 * g;
        vals.push(v);
        total[i] += v;
      }
      bars(row.querySelector("canvas"), vals, COLORS[name]);
    });
    bars(sum, total, "#ffffff");
  }

  rows.forEach((row) => {
    const input = row.querySelector("input");
    const out = row.querySelector("output");
    const mute = row.querySelector(".mute");
    input.addEventListener("input", () => {
      const v = Number(input.value);
      out.textContent = (v > 0 ? "+" : "") + v + " dB";
      render();
    });
    mute.addEventListener("click", () => {
      mute.setAttribute("aria-pressed", mute.getAttribute("aria-pressed") === "true" ? "false" : "true");
      render();
    });
  });

  render();
  let rt = 0;
  addEventListener("resize", () => { cancelAnimationFrame(rt); rt = requestAnimationFrame(render); });
})();

/* ---------- The sliding mark in the nav, like the app's ---------- */

(function nav() {
  const nav = document.querySelector(".pillnav");
  if (!nav) return;
  const mark = nav.querySelector(".pillnav-mark");
  const links = [...nav.querySelectorAll('a[href^="#"]')];
  let current = null;

  function moveTo(a) {
    if (!a) { mark.style.opacity = "0"; return; }
    mark.style.width = a.offsetWidth + "px";
    mark.style.transform = `translateX(${a.offsetLeft}px)`;
    mark.style.opacity = "1";
  }
  function setCurrent(a) {
    current = a;
    links.forEach((l) => (l === a ? l.setAttribute("aria-current", "true") : l.removeAttribute("aria-current")));
    moveTo(a);
  }

  links.forEach((a) => {
    a.addEventListener("mouseenter", () => moveTo(a));
  });
  nav.addEventListener("mouseleave", () => moveTo(current));

  const sections = links.map((a) => document.querySelector(a.getAttribute("href")));
  const seen = new Map();
  const io = new IntersectionObserver((entries) => {
    entries.forEach((e) => seen.set(e.target, e.isIntersecting));
    const i = sections.findIndex((sec) => seen.get(sec));
    setCurrent(i >= 0 ? links[i] : null);
  }, { rootMargin: "-45% 0px -50% 0px" });
  sections.forEach((sec) => sec && io.observe(sec));
})();

/* ---------- Latest release from GitHub ---------- */

(async function release() {
  const KEY = "clippiboy-release";
  let data = null;
  try { data = JSON.parse(sessionStorage.getItem(KEY)); } catch {}
  if (!data) {
    try {
      const res = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`, {
        headers: { Accept: "application/vnd.github+json" },
      });
      if (!res.ok) return;
      data = await res.json();
      try { sessionStorage.setItem(KEY, JSON.stringify(data)); } catch {}
    } catch {
      return; // the links already point at the release page
    }
  }

  const assets = data.assets || [];
  const setup = assets.find((a) => /-setup\.exe$/i.test(a.name));
  const plugin = assets.find((a) => /\.streamDeckPlugin$/i.test(a.name));
  const version = String(data.tag_name || "").replace(/^v/, "");

  if (setup) {
    document.querySelectorAll("[data-download]").forEach((a) => { a.href = setup.browser_download_url; });
  }
  if (plugin) {
    document.querySelectorAll("[data-plugin]").forEach((a) => { a.href = plugin.browser_download_url; });
  }
  if (version) {
    const mb = setup ? `, ${(setup.size / 1048576).toFixed(1)} MB` : "";
    document.querySelectorAll("[data-version]").forEach((el) => { el.textContent = `Version ${version}${mb}`; });
  }

  const target = document.querySelector("[data-release]");
  if (target && version) {
    const date = data.published_at
      ? new Date(data.published_at).toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric" })
      : "";
    const link = document.createElement("a");
    link.href = data.html_url;
    link.textContent = "What's new";
    target.replaceChildren(
      `ClippiBoy ${version}${date ? `, released ${date}` : ""}. `,
      link,
    );
  }
})();
