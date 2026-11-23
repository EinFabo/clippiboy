import { test } from "node:test";
import assert from "node:assert/strict";
import { DOWNLOADS, rewriteLocation, SITE_CSP, validRelease, withSecurityHeaders } from "../site-proxy/rules.ts";

const setup = `${DOWNLOADS}v1.0.0/ClippiBoy_1.0.0_x64-setup.exe`;

test("a release of ours passes, with its version and setup", () => {
  assert.deepEqual(validRelease({ version: "1.0.0", setup: { url: setup, size: 1 } }), { version: "1.0.0", url: setup });
});

test("a setup from anywhere else is not fetched", () => {
  for (const url of [
    "https://evil.example/ClippiBoy.exe",
    "https://github.com/someone/clippiboy/releases/download/v1.0.0/x.exe",
    "http://github.com/EinFabo/clippiboy/releases/download/v1.0.0/x.exe",
    "https://github.com.evil.example/EinFabo/clippiboy/releases/download/v1.0.0/x.exe",
    "https://user@github.com/EinFabo/clippiboy/releases/download/v1.0.0/x.exe",
    `${DOWNLOADS}../../../../other/repo/x.exe`,
    "not a url",
  ]) {
    assert.equal(validRelease({ version: "1.0.0", setup: { url } }), null, url);
  }
  assert.equal(validRelease({ version: "1.0.0" }), null);
  assert.equal(validRelease({ version: "1.0.0", setup: { url: 5 } }), null);
  assert.equal(validRelease(null), null);
});

test("the version goes into a file name only as plain numbers", () => {
  for (const version of ['1.0.0"; x="', "1.0", "1.0.0-beta", "v1.0.0", "1.0.0\r\nX: y", ""]) {
    assert.equal(validRelease({ version, setup: { url: setup } }), null, version);
  }
  assert.equal(validRelease({ version: 100, setup: { url: setup } }), null);
});

test("GitHub's redirects come back to clippiboy.com", () => {
  assert.equal(rewriteLocation("https://einfabo.github.io/clippiboy/img/"), "https://clippiboy.com/img/");
  assert.equal(rewriteLocation("https://einfabo.github.io/clippiboy"), "https://clippiboy.com/");
  assert.equal(rewriteLocation("/clippiboy/img/"), "/img/");
  assert.equal(rewriteLocation("/clippiboy"), "/");
  assert.equal(rewriteLocation("/clippiboy?x=1"), "/?x=1");
});

test("a rewritten redirect never leaves the site", () => {
  assert.equal(rewriteLocation("/clippiboy//evil.example"), "/evil.example");
  assert.equal(rewriteLocation("/clippiboy/\\evil.example"), "/evil.example");
  assert.equal(rewriteLocation("/clippiboy///evil.example/x"), "/evil.example/x");
  assert.equal(rewriteLocation("https://einfabo.github.io/clippiboy//evil.example"), "https://clippiboy.com/evil.example");
  // Not under the site: left alone.
  assert.equal(rewriteLocation("/clippiboyfoo"), "/clippiboyfoo");
  assert.equal(rewriteLocation("https://github.com/EinFabo"), "https://github.com/EinFabo");
});

test("pages get a policy, nosniff and a referrer policy", () => {
  const headers = withSecurityHeaders(new Headers({ "Content-Type": "text/html" }));
  assert.equal(headers.get("Content-Security-Policy"), SITE_CSP);
  assert.equal(headers.get("X-Content-Type-Options"), "nosniff");
  assert.equal(headers.get("Referrer-Policy"), "strict-origin-when-cross-origin");
  assert.match(SITE_CSP, /frame-ancestors 'none'/);
  assert.match(SITE_CSP, /script-src 'self'(;|$)/);
  assert.doesNotMatch(SITE_CSP, /unsafe-inline|unsafe-eval/);
  // What main.js asks for.
  assert.match(SITE_CSP, /connect-src 'self' https:\/\/api\.github\.com/);
});
