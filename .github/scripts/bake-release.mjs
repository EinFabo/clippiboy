// Writes the latest release into the page before it is published.
//
// Run by pages.yml. Without this the page only learned the version in the
// browser, from the GitHub API: 60 requests an hour per address, nothing at all
// without JavaScript (link previews, search engines), and a tab that loaded
// while a release was still uploading kept a release without an installer.
//
// Produces `site/release.json` for main.js and fills the same values into
// `site/index.html`, so the page is right before any script has run. If GitHub
// cannot be reached the page is left as it is — its links point at the release
// page and still work — and the deploy goes ahead.

import { readFile, writeFile } from "node:fs/promises";

const repo = process.env.GITHUB_REPOSITORY ?? "EinFabo/clippiboy";
const token = process.env.GH_TOKEN;

async function releases() {
  const all = [];
  for (let page = 1; page <= 10; page++) {
    const res = await fetch(
      `https://api.github.com/repos/${repo}/releases?per_page=100&page=${page}`,
      {
        headers: {
          Accept: "application/vnd.github+json",
          ...(token ? { Authorization: `Bearer ${token}` } : {}),
        },
      },
    );
    if (!res.ok) throw new Error(`GitHub answered ${res.status}`);
    const batch = await res.json();
    all.push(...batch);
    if (batch.length < 100) break;
  }
  // App releases only: ffmpeg has releases of its own in the same repository.
  return all.filter((r) => !r.draft && !r.prerelease && /^v\d/.test(r.tag_name));
}

const escape = (text) =>
  String(text).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

let list;
try {
  list = await releases();
} catch (err) {
  console.warn(`::warning::Release not baked into the page: ${err.message}`);
  process.exit(0);
}

const isSetup = (a) => /-setup\.exe$/i.test(a.name);
const latest = list
  .filter((r) => r.assets.some(isSetup))
  .sort((a, b) => Date.parse(b.published_at) - Date.parse(a.published_at))[0];
if (!latest) {
  console.warn("::warning::No release with an installer yet — page left as it is.");
  process.exit(0);
}

const setup = latest.assets.find(isSetup);
const plugin = latest.assets.find((a) => /\.streamDeckPlugin$/i.test(a.name));
// Every installer ever downloaded, not just this version's — otherwise the count
// starts over at every release. The `.sig` and `latest.json` beside it are
// fetched by the updater, not by people, and are left out.
const downloads = list
  .flatMap((r) => r.assets)
  .filter(isSetup)
  .reduce((sum, a) => sum + a.download_count, 0);

const release = {
  version: latest.tag_name.replace(/^v/, ""),
  publishedAt: latest.published_at,
  url: latest.html_url,
  setup: { url: setup.browser_download_url, size: setup.size },
  plugin: plugin ? { url: plugin.browser_download_url } : null,
  downloads,
};
await writeFile("site/release.json", JSON.stringify(release, null, 2) + "\n");

// The same wording main.js uses — see `show` there.
const mb = (release.setup.size / 1048576).toFixed(1);
const date = new Date(release.publishedAt).toLocaleDateString("en-GB", {
  day: "numeric",
  month: "long",
  year: "numeric",
  timeZone: "UTC",
});
const count = downloads.toLocaleString("en-US");

let html = await readFile("site/index.html", "utf8");
const swap = (pattern, replacement) => {
  const next = html.replace(pattern, replacement);
  if (next === html) console.warn(`::warning::Nothing in index.html matched ${pattern}`);
  html = next;
};
swap(/(data-download href=")[^"]*"/g, `$1${escape(release.setup.url)}"`);
if (release.plugin) swap(/(data-plugin href=")[^"]*"/g, `$1${escape(release.plugin.url)}"`);
swap(/(<span data-version>)[^<]*(<\/span>)/, `$1Version ${escape(release.version)}, ${mb} MB$2`);
if (downloads > 0) {
  swap(/<span data-downloads hidden>[^<]*<\/span>/, `<span data-downloads> · ${count} downloads</span>`);
}
swap(
  /(<p data-release>)[\s\S]*?(<\/p>)/,
  `$1ClippiBoy ${escape(release.version)}, released ${date}. ` +
    `<a href="${escape(release.url)}">What's new</a>$2`,
);
await writeFile("site/index.html", html);

console.log(`Baked ${release.version} (${mb} MB, ${count} downloads) into the page.`);
