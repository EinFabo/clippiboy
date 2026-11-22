import { test } from "node:test";
import assert from "node:assert/strict";
import { layoutMonitors, numberedMonitors, placeGhost, orderedWindows, rectOf, sameRect } from "./monitorMap.ts";
import type { CaptureTarget } from "./types.ts";

const primary = { x: 0, y: 0, width: 2560, height: 1440 };

test("nothing to draw gives nothing", () => {
  assert.deepEqual(layoutMonitors([], { width: 500, height: 200 }), []);
  assert.deepEqual(layoutMonitors([primary], { width: 0, height: 200 }), []);
});

test("a single screen fills the area at its own aspect, centred", () => {
  const [box] = layoutMonitors([primary], { width: 400, height: 300 }, 0);
  // 400 / 2560 is the tighter scale: full width, 225 high, 37.5 down.
  assert.deepEqual(box, { left: 0, top: 37.5, width: 400, height: 225 });
});

test("a screen left of the primary one, at negative x, comes out on the left", () => {
  const second = { x: -1920, y: 180, width: 1920, height: 1080 };
  const [main, side] = layoutMonitors([primary, second], { width: 560, height: 220 });
  // 4480 px wide → scale 0.125; 180 px high → 20 down.
  assert.deepEqual(side, { left: 3, top: 45.5, width: 234, height: 129 });
  assert.deepEqual(main, { left: 243, top: 23, width: 314, height: 174 });
  // The two touch on the desktop and stand 6 px apart here.
  assert.equal(main.left - (side.left + side.width), 6);
});

test("a stack keeps one screen above the other", () => {
  const above = { x: 0, y: -1080, width: 1920, height: 1080 };
  const [main, top] = layoutMonitors([primary, above], { width: 300, height: 300 }, 0);
  assert.ok(top.top + top.height <= main.top + 0.01);
  // Scaled alike: the 1920 screen is three quarters the width of the 2560 one.
  assert.ok(Math.abs(top.width / main.width - 0.75) < 0.01);
  assert.ok(main.top + main.height <= 300);
});

test("different sizes keep their proportions", () => {
  const small = { x: 2560, y: 0, width: 1280, height: 1024 };
  const [main, side] = layoutMonitors([primary, small], { width: 1000, height: 1000 }, 0);
  assert.ok(Math.abs(main.width / main.height - 2560 / 1440) < 0.01);
  assert.ok(Math.abs(side.width / side.height - 1280 / 1024) < 0.01);
  assert.ok(Math.abs(side.width / main.width - 0.5) < 0.01);
});

const target = (over: Partial<CaptureTarget>): CaptureTarget => ({
  kind: "monitor",
  id: "x",
  stableId: null,
  title: "",
  width: 1920,
  height: 1080,
  isPrimary: false,
  refreshHz: 60,
  x: 0,
  y: 0,
  icon: null,
  isGame: false,
  ...over,
});

test("the primary screen is number 1 wherever Windows lists it", () => {
  const list = [
    target({ id: "a" }),
    target({ id: "b", isPrimary: true }),
    target({ id: "w", kind: "window" }),
    target({ id: "c" }),
  ];
  assert.deepEqual(numberedMonitors(list).map((t) => t.id), ["b", "a", "c"]);
});

test("the detected game leads the windows", () => {
  const list = [
    target({ id: "m" }),
    target({ id: "chat", kind: "window" }),
    target({ id: "game", kind: "window", isGame: true }),
  ];
  assert.deepEqual(orderedWindows(list).map((t) => t.id), ["game", "chat"]);
});

test("a monitor's rect is what the config keeps, a window has none", () => {
  const screen = target({ x: -1920, y: 180 });
  assert.deepEqual(rectOf(screen), { x: -1920, y: 180, width: 1920, height: 1080 });
  assert.equal(rectOf(target({ kind: "window" })), null);
  assert.ok(sameRect(rectOf(screen), { x: -1920, y: 180, width: 1920, height: 1080 }));
  assert.ok(!sameRect(rectOf(screen), null));
  assert.ok(sameRect(null, null));
});

test("an unplugged screen keeps its place when nothing has moved there", () => {
  const ghost = { x: -1920, y: 180, width: 1920, height: 1080 };
  assert.deepEqual(placeGhost([primary], ghost), ghost);
});

test("an unplugged screen whose place a live one took stands beside them instead", () => {
  // The old primary is gone; the second screen became primary at 0,0.
  const live = [{ x: 0, y: 0, width: 1920, height: 1080 }];
  const ghost = { x: 0, y: 0, width: 2560, height: 1440 };
  assert.deepEqual(placeGhost(live, ghost), { x: 1920, y: 0, width: 2560, height: 1440 });
});
