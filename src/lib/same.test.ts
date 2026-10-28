import { test } from "node:test";
import assert from "node:assert/strict";
import { sameRecord } from "./same.ts";

test("two empty maps are the same", () => assert.equal(sameRecord({}, {}), true));
test("equal maps are the same", () => assert.equal(sameRecord({ a: "x", b: "y" }, { b: "y", a: "x" }), true));
test("a changed message is not", () => assert.equal(sameRecord({ a: "x" }, { a: "y" }), false));
test("an extra key is not", () => assert.equal(sameRecord({ a: "x" }, { a: "x", b: "y" }), false));
test("a different key is not", () => assert.equal(sameRecord({ a: "x" }, { b: "x" }), false));
