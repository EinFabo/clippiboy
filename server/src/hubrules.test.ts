import { test } from "node:test";
import assert from "node:assert/strict";
import { CONTACT_MAX, CONTACT_MS, mayReach, remember, type Contacts } from "./hubrules.ts";

const now = 1_000_000;

test("an invisible user can be reached only by friends they wrote to in the last ten minutes", () => {
  const contacts = remember({}, "f1", now);
  assert.equal(mayReach(contacts, "f1", now + CONTACT_MS - 1), true);
  assert.equal(mayReach(contacts, "f1", now + CONTACT_MS), false);
  assert.equal(mayReach(contacts, "f2", now), false);
  assert.equal(mayReach(undefined, "f1", now), false);
});

test("the list drops what expired and keeps the newest fifty", () => {
  let contacts: Contacts = { old: now - 1 };
  for (let i = 0; i < 60; i++) contacts = remember(contacts, `f${i}`, now + i);
  assert.equal(Object.keys(contacts).length, CONTACT_MAX);
  assert.equal("old" in contacts || "f0" in contacts, false);
  assert.equal("f59" in contacts, true);
});
