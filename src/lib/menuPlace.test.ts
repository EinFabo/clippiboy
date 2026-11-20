import { test } from "node:test";
import assert from "node:assert/strict";
import { placeMenu } from "./menuPlace.ts";

const viewport = { width: 1080, height: 680 };
const base = { viewport, margin: 8, gap: 0, alignRight: false };
const rect = (left: number, top: number, right = left, bottom = top) => ({ left, top, right, bottom });

test("fits below: opens at the anchor's bottom, no cap needed", () => {
  const p = placeMenu({ ...base, anchor: rect(100, 100, 200, 130), size: { width: 224, height: 200 }, gap: 6 });
  assert.equal(p.placement, "below");
  assert.equal(p.y, 136);
  assert.equal(p.x, 100);
  assert.equal(p.maxHeight, 680 - 136 - 8);
});

test("does not fit below but fits above: flips, bottom edge sits at the anchor's top", () => {
  const p = placeMenu({ ...base, anchor: rect(100, 600, 200, 630), size: { width: 224, height: 300 }, gap: 6 });
  assert.equal(p.placement, "above");
  assert.equal(p.y, 600 - 6 - 300);
  assert.equal(p.maxHeight, 600 - 6 - 8);
});

test("neither fits: the larger side, height capped to it minus the margin", () => {
  const p = placeMenu({ ...base, anchor: rect(100, 300, 100, 300), size: { width: 224, height: 900 } });
  // above has 300 - 8 = 292, below has 680 - 300 - 8 = 372
  assert.equal(p.placement, "below");
  assert.equal(p.maxHeight, 372);
  assert.equal(p.y, 300);
  const q = placeMenu({ ...base, anchor: rect(100, 400, 100, 400), size: { width: 224, height: 900 } });
  assert.equal(q.placement, "above");
  assert.equal(q.maxHeight, 392);
  assert.equal(q.y, 8);
});

test("a menu taller than the whole window fills it between the margins", () => {
  const p = placeMenu({ ...base, anchor: rect(10, 340), size: { width: 224, height: 5000 } });
  assert.ok(p.y >= 8);
  assert.ok(p.y + p.maxHeight <= 680 - 8);
});

test("never leaves the window horizontally", () => {
  assert.equal(placeMenu({ ...base, anchor: rect(1070, 50), size: { width: 224, height: 100 } }).x, 1080 - 224 - 8);
  assert.equal(placeMenu({ ...base, anchor: rect(-30, 50), size: { width: 224, height: 100 } }).x, 8);
  assert.equal(placeMenu({ ...base, alignRight: true, anchor: rect(0, 50, 100, 80), size: { width: 224, height: 100 } }).x, 8);
});

test("alignRight puts the menu's right edge on the anchor's right edge", () => {
  const p = placeMenu({ ...base, alignRight: true, anchor: rect(700, 50, 900, 80), size: { width: 224, height: 100 } });
  assert.equal(p.x, 900 - 224);
});

test("the result stays inside the window for every anchor and size", () => {
  for (let ax = -20; ax <= 1100; ax += 77)
    for (let ay = -20; ay <= 700; ay += 53)
      for (const height of [40, 300, 640, 2000]) {
        const p = placeMenu({ ...base, anchor: rect(ax, ay), size: { width: 224, height } });
        const used = Math.min(height, p.maxHeight);
        assert.ok(p.x >= 8 && p.x + 224 <= 1080 - 8);
        assert.ok(p.y >= 8 && p.y + used <= 680 - 8, `${ax},${ay},${height}`);
      }
});
