import { test } from "node:test";
import assert from "node:assert/strict";
import { camBox, layoutFromRect, sameFacecam, snapLayout } from "./camGeometry.ts";
import type { CamLayout } from "./types.ts";

const base: CamLayout = { visible: true, x: 1, y: 1, size: 0.25, shape: "rounded", mirror: false };

test("layout round-trips through pixels", () => {
  for (const size of [0.12, 0.25, 0.5]) {
    for (const [x, y] of [[0, 0], [1, 1], [0.5, 0.5], [0.3, 0.8]]) {
      const l = { ...base, x, y, size };
      const b = camBox(l, 1920, 1080);
      const back = layoutFromRect({ x: b.left, y: b.top, width: b.width, height: b.height }, { width: 1920, height: 1080 }, l.shape);
      assert.ok(Math.abs(back.size - size) <= 0.002, `size ${size}: ${back.size}`);
      assert.ok(Math.abs(back.x - x) <= 0.002, `x ${x} at ${size}: ${back.x}`);
      assert.ok(Math.abs(back.y - y) <= 0.002, `y ${y} at ${size}: ${back.y}`);
    }
  }
});
test("a box near a corner snaps into it", () => {
  assert.deepEqual(snapLayout({ ...base, x: 0.98, y: 0.02 }), { ...base, x: 1, y: 0 });
  assert.deepEqual(snapLayout({ ...base, x: 0.5, y: 0.5 }), { ...base, x: 0.5, y: 0.5 });
});
test("the size stays between 12 % and 50 %", () => {
  const tiny = layoutFromRect({ x: 0, y: 0, width: 20, height: 10 }, { width: 1920, height: 1080 }, "rect");
  assert.equal(tiny.size, 0.12);
  const huge = layoutFromRect({ x: 0, y: 0, width: 1900, height: 1000 }, { width: 1920, height: 1080 }, "rect");
  assert.equal(huge.size, 0.5);
});

test("same facecam ignores moves inside the tolerance", () => {
  const a = { layout: base, delayMs: -40 };
  assert.equal(sameFacecam(a, { layout: { ...base, x: base.x - 0.001 }, delayMs: -40 }), true);
  assert.equal(sameFacecam(a, { layout: { ...base, x: base.x - 0.003 }, delayMs: -40 }), false);
  assert.equal(sameFacecam(a, { layout: base, delayMs: -30 }), false);
  assert.equal(sameFacecam(a, { layout: { ...base, mirror: true }, delayMs: -40 }), false);
  assert.equal(sameFacecam(a, { layout: { ...base, visible: false }, delayMs: -40 }), false);
});
