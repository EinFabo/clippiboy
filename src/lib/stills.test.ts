import { test } from "node:test";
import assert from "node:assert/strict";
import { shouldFetchStills, stillsById } from "./stills.ts";

test("stills are asked for while the page is seen and nothing is out", () => {
  assert.equal(shouldFetchStills({ visible: true, focused: true, inFlight: false, screens: 2 }), true);
});

test("a request still out skips the tick instead of piling up", () => {
  assert.equal(shouldFetchStills({ visible: true, focused: true, inFlight: true, screens: 2 }), false);
});

test("nothing is asked for a hidden page or without screens", () => {
  assert.equal(shouldFetchStills({ visible: false, focused: true, inFlight: false, screens: 2 }), false);
  assert.equal(shouldFetchStills({ visible: true, focused: true, inFlight: false, screens: 0 }), false);
});

test("another window in front, a game say, gets no stills taken", () => {
  assert.equal(shouldFetchStills({ visible: true, focused: false, inFlight: false, screens: 2 }), false);
});

test("screens without a still are left out of the map", () => {
  assert.deepEqual(
    stillsById([
      { id: "\\\\.\\DISPLAY1", jpeg: "data:image/jpeg;base64,AAA" },
      { id: "\\\\.\\DISPLAY2", jpeg: null },
    ]),
    { "\\\\.\\DISPLAY1": "data:image/jpeg;base64,AAA" },
  );
  assert.deepEqual(stillsById([]), {});
});
