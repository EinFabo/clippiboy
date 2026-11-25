import { test } from "node:test";
import assert from "node:assert/strict";
import { freeOn, linkReason, shareAsLinkLabel, type LinkFacts } from "./linkReason.ts";
import type { LinkProgress, LinkQuota } from "./links.ts";

const NOW = 1_000_000;
const facts = (over: Partial<LinkFacts> = {}): LinkFacts => ({
  links: {},
  signedIn: true,
  quota: { used: 1, limit: 5, resetsAt: NOW + 86_400_000, full: false },
  uploads: {},
  ...over,
});
const upload = (stage: LinkProgress["stage"]): Record<string, LinkProgress> => ({
  c: { clipId: "c", title: "Ace", stage, progress: 0.5, error: null },
});
const usedUp: LinkQuota = { used: 5, limit: 5, resetsAt: NOW + 86_400_000, full: false };

test("link: one to be had says how long it lasts and can be clicked", () => {
  const ok = linkReason("c", facts(), NOW, true);
  assert.deepEqual(ok, { live: null, reason: null, enabled: true });
  assert.equal(shareAsLinkLabel(ok.reason), "Share as link (5 days)");
});

test("link: a live one is copied again, one run out is gone", () => {
  const live = { id: "l", url: "https://clippiboy.com/c/l", expiresAt: NOW + 1 };
  assert.equal(linkReason("c", facts({ links: { c: live } }), NOW, true).live, live);
  const old = { ...live, expiresAt: NOW };
  assert.equal(linkReason("c", facts({ links: { c: old } }), NOW, true).live, null);
});

test("link: on its way up, signed out or sharing full — not clickable, the label says why", () => {
  const up = linkReason("c", facts({ uploads: upload("uploading") }), NOW, true);
  assert.deepEqual([up.reason, up.enabled], ["uploading…", false]);
  assert.equal(shareAsLinkLabel(up.reason), "Share as link · uploading…");
  assert.equal(linkReason("c", facts({ uploads: upload("shrinking") }), NOW, true).reason, "uploading…");
  // A finished upload of this clip, or one of another clip, is no reason.
  assert.equal(linkReason("c", facts({ uploads: upload("failed") }), NOW, true).reason, null);
  assert.equal(linkReason("d", facts({ uploads: upload("uploading") }), NOW, true).reason, null);
  const out = linkReason("c", facts({ signedIn: false }), NOW, true);
  assert.deepEqual([out.reason, out.enabled], ["sign in on Friends first", false]);
  const full = linkReason("c", facts({ quota: { used: 1, limit: 5, resetsAt: null, full: true } }), NOW, true);
  assert.deepEqual([full.reason, full.enabled], ["sharing is full right now", false]);
});

test("link: a used-up week says until when, and may still be tried", () => {
  const week = linkReason("c", facts({ quota: usedUp }), NOW, true);
  assert.equal(week.reason, `none left until ${freeOn(usedUp)}`);
  assert.equal(week.enabled, true);
  // Not while this clip's own upload is what used it up.
  const mine = linkReason("c", facts({ quota: usedUp, uploads: upload("uploading") }), NOW, true);
  assert.deepEqual([mine.reason, mine.enabled], ["uploading…", false]);
  // Without a reset date: next week.
  assert.equal(linkReason("c", facts({ quota: { ...usedUp, resetsAt: null } }), NOW, true).reason, "none left until next week");
  // A week that has reset since the store last heard counts as fresh.
  const reset = linkReason("c", facts({ quota: { ...usedUp, resetsAt: NOW } }), NOW, true);
  assert.deepEqual([reset.reason, reset.enabled], [null, true]);
});

test("link: in the browser nothing goes up", () => {
  assert.deepEqual(linkReason("c", facts({ signedIn: false }), NOW, false), { live: null, reason: null, enabled: false });
});
