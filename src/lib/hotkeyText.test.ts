import { test } from "node:test";
import assert from "node:assert/strict";
import type { AppConfig, PadBindings, PadProfiles } from "./types.ts";
import {
  anyPadBinding,
  canRecord,
  editProfile,
  openingTab,
  profileTabs,
  resetProfile,
  shownHotkey,
  showsControllers,
  tabBindings,
  hotkeyKeys,
  hotkeyLabel,
  isHold,
  isPad,
  keyLabel,
  padButton,
  padLabel,
  placeBinding,
  switchMode,
  triggerKey,
  triggerOf,
  withHold,
} from "./hotkeyText.ts";

test("an old keyboard binding reads as before", () => {
  assert.deepEqual(hotkeyKeys("Ctrl+Shift+S"), ["Ctrl", "Shift", "S"]);
  assert.equal(hotkeyLabel("Alt+PrintScreen"), "Alt + PrtSc");
  assert.equal(hotkeyLabel("Numpad1"), "Num 1");
  assert.equal(isHold("Ctrl+Shift+S"), false);
  assert.equal(hotkeyLabel(""), "");
});

test("mouse buttons get their printed names", () => {
  assert.equal(keyLabel("Mouse4"), "Mouse 4");
  assert.equal(hotkeyLabel("Ctrl+Mouse5"), "Ctrl + Mouse 5");
  assert.equal(isPad("Mouse3"), false);
});

test("controller combos drop the prefix", () => {
  assert.equal(isPad("Pad:View+A"), true);
  assert.equal(isPad("Hold:Pad:LB+RB+South"), true);
  assert.deepEqual(hotkeyKeys("Pad:LB+RB+South"), ["LB", "RB", "A"]);
  assert.equal(hotkeyLabel("Hold:Pad:Back+South"), "Hold View + A");
});

test("old bindings in Xbox names read as before", () => {
  assert.deepEqual(hotkeyKeys("Pad:LB+RB+A"), ["LB", "RB", "A"]);
  assert.equal(hotkeyLabel("Hold:Pad:View+A"), "Hold View + A");
  assert.equal(padButton("View"), "Back");
  assert.equal(padButton("a"), "South");
  assert.equal(padButton("Xbox"), "Guide");
});

test("a single button and Guide read on their own", () => {
  assert.deepEqual(hotkeyKeys("Pad:Guide"), ["Xbox"]);
  assert.equal(hotkeyLabel("Hold:Pad:Misc"), "Hold Share");
  assert.equal(hotkeyLabel("Pad:Up"), "Up");
});

test("buttons carry the names printed on the pad in use", () => {
  assert.equal(hotkeyLabel("Pad:Back+South", "playstation"), "Share + Cross");
  assert.equal(hotkeyLabel("Pad:Guide", "playstation"), "PS");
  assert.equal(hotkeyLabel("Pad:LB+RT", "playstation"), "L1 + R2");
  // By position: the bottom button is B on a Nintendo pad, the right one A.
  assert.equal(padLabel("South", "nintendo"), "B");
  assert.equal(padLabel("East", "nintendo"), "A");
  assert.equal(padLabel("A", "nintendo"), "B");
  assert.equal(hotkeyLabel("Pad:Misc", "nintendo"), "Capture");
  assert.equal(padLabel("Guide", "nintendo"), "Home");
  assert.equal(hotkeyLabel("Pad:East+West", "gamecube"), "X + B");
  // What a family has no name of its own for, Xbox's name stands in.
  assert.equal(padLabel("Touchpad", "nintendo"), "Touchpad");
  assert.equal(padLabel("LS", "nintendo"), "LS");
});

test("tap and hold switch without touching what is pressed", () => {
  assert.equal(isHold("Hold:Mouse5"), true);
  assert.equal(isHold("hold: Mouse5"), true);
  assert.equal(triggerOf("Hold:Mouse5"), "Mouse5");
  assert.equal(withHold("Mouse5", true), "Hold:Mouse5");
  assert.equal(withHold("Hold:Mouse5", false), "Mouse5");
  assert.equal(withHold("Hold:Ctrl+S", true), "Hold:Ctrl+S");
  assert.equal(withHold("", true), "");
});

test("one input in two spellings is the same input", () => {
  assert.equal(triggerKey("ctrl+s"), triggerKey("Hold:Ctrl+S"));
  assert.equal(triggerKey("Control+KeyS"), triggerKey("Ctrl+S"));
  assert.equal(triggerKey("Pad:View+A"), triggerKey("Hold:Pad:Back+South"));
  assert.equal(triggerKey("Shift+Ctrl+Mouse5"), triggerKey("Ctrl+Shift+Mouse5"));
  assert.notEqual(triggerKey("Ctrl+Mouse5"), triggerKey("Mouse5"));
  assert.notEqual(triggerKey("Pad:South"), triggerKey("Pad:Back+South"));
  assert.equal(triggerKey(""), "");
});

const bound = (changes: Record<string, string>) => ({
  saveClip: "",
  screenshot: "",
  record: "",
  screenshotAlt: "",
  ...changes,
});

test("a second action on the same input takes the free way", () => {
  // Mouse5 tapped saves a clip: recorded for the screenshot, it becomes held.
  assert.deepEqual(placeBinding(bound({ saveClip: "Mouse5" }), "screenshot", "Mouse5"), {
    screenshot: "Hold:Mouse5",
  });
  // And the other way round, in another spelling, in the second slot.
  assert.deepEqual(
    placeBinding(bound({ saveClip: "Hold:Pad:View+A" }), "screenshotAlt", "Pad:Back+South"),
    { screenshotAlt: "Pad:Back+South" },
  );
  assert.deepEqual(
    placeBinding(bound({ saveClip: "Pad:Back+South" }), "screenshotAlt", "Pad:View+A"),
    { screenshotAlt: "Hold:Pad:View+A" },
  );
  // A free input, or the same binding recorded again on its own field: as it is.
  assert.deepEqual(placeBinding(bound({ saveClip: "Mouse4" }), "screenshot", "Mouse5"), {
    screenshot: "Mouse5",
  });
  assert.deepEqual(placeBinding(bound({ saveClip: "Mouse5" }), "saveClip", "Mouse5"), {
    saveClip: "Mouse5",
  });
});

test("the same input in an action's own second slot stays as recorded — a clash", () => {
  const slots = { saveClipHotkey: "Mouse5", saveClipHotkeyAlt: "", screenshotHotkey: "" };
  assert.deepEqual(placeBinding(slots, "saveClipHotkeyAlt", "Mouse5"), { saveClipHotkeyAlt: "Mouse5" });
  const back = { saveClipHotkey: "", saveClipHotkeyAlt: "Pad:Back+South", screenshotHotkey: "" };
  assert.deepEqual(placeBinding(back, "saveClipHotkey", "Pad:View+A"), { saveClipHotkey: "Pad:View+A" });
  // Another action on it as well: still no flip onto the action's own input.
  const shared = { saveClipHotkey: "Mouse5", saveClipHotkeyAlt: "", screenshotHotkey: "Mouse5" };
  assert.deepEqual(placeBinding(shared, "saveClipHotkeyAlt", "Mouse5"), { saveClipHotkeyAlt: "Mouse5" });
});

test("a third action on an input with both ways taken stays a clash", () => {
  const both = bound({ saveClip: "Mouse5", screenshot: "Hold:Mouse5" });
  assert.deepEqual(placeBinding(both, "record", "Mouse5"), { record: "Mouse5" });
  assert.deepEqual(placeBinding(both, "record", "Hold:Mouse5"), { record: "Hold:Mouse5" });
});

test("switching tap and hold swaps with the action sharing the input", () => {
  const pair = bound({ saveClip: "Mouse5", screenshot: "Hold:Mouse5" });
  assert.deepEqual(switchMode(pair, "saveClip", true), {
    saveClip: "Hold:Mouse5",
    screenshot: "Mouse5",
  });
  assert.deepEqual(switchMode(pair, "screenshot", false), {
    screenshot: "Mouse5",
    saveClip: "Hold:Mouse5",
  });
  // Alone on its input: only itself; already that way: nothing.
  assert.deepEqual(switchMode(bound({ record: "F9" }), "record", true), { record: "Hold:F9" });
  assert.deepEqual(switchMode(pair, "saveClip", false), {});
});

const none: PadBindings = { saveClip: "", toggleBuffer: "", screenshot: "", record: "", console: "" };
const profiles = (over: Partial<PadProfiles> = {}): PadProfiles => ({
  all: none,
  xbox: null,
  playstation: null,
  nintendo: null,
  ...over,
});

test("without a controller there is no controller section", () => {
  assert.equal(showsControllers(profiles(), [], []), false);
  // Seen once, plugged in now, or something bound: it shows, and stays.
  assert.equal(showsControllers(profiles(), ["xbox"], []), true);
  assert.equal(showsControllers(profiles(), [], ["other"]), true);
  const bound = profiles({ all: { ...none, saveClip: "Pad:Guide" } });
  assert.equal(anyPadBinding(bound), true);
  assert.equal(showsControllers(bound, [], []), true);
});

test("the profile tabs show only once there is more than one kind of pad", () => {
  // One Xbox pad: no tabs, the section edits All.
  assert.deepEqual(profileTabs(profiles(), ["xbox"], ["xbox"]), { tabs: ["all"], selector: false });
  // Xbox and PlayStation seen: a tab each, never one for a type never seen.
  assert.deepEqual(profileTabs(profiles(), ["xbox", "playstation"], []), {
    tabs: ["all", "xbox", "playstation"],
    selector: true,
  });
  // Another pad beside an Xbox one has no tab, but makes the tabs show.
  assert.deepEqual(profileTabs(profiles(), ["xbox"], ["other"]).tabs, ["all", "xbox"]);
  // A type with a profile of its own always has its tab.
  assert.deepEqual(profileTabs(profiles({ nintendo: none }), ["xbox"], []), {
    tabs: ["all", "xbox", "nintendo"],
    selector: true,
  });
});

test("the section opens on the profile the pad in use goes by", () => {
  const tabs = ["all", "xbox", "playstation"] as const;
  assert.equal(openingTab(profiles(), "xbox", [...tabs]), "all");
  assert.equal(openingTab(profiles({ xbox: none }), "xbox", [...tabs]), "xbox");
  assert.equal(openingTab(profiles({ xbox: none }), "playstation", [...tabs]), "all");
  assert.equal(openingTab(profiles({ xbox: none }), null, [...tabs]), "all");
  assert.equal(openingTab(profiles(), "other", [...tabs]), "all");
});

test("a type without a profile of its own shows All's bindings, borrowed", () => {
  const all = { ...none, screenshot: "Pad:Guide" };
  const shown = tabBindings(profiles({ all }), "playstation");
  assert.equal(shown.inherited, true);
  assert.equal(shown.bindings.screenshot, "Pad:Guide");
  // In that type's names.
  assert.deepEqual(hotkeyKeys(shown.bindings.screenshot, "playstation"), ["PS"]);
  assert.equal(tabBindings(profiles({ all }), "all").inherited, false);
  assert.equal(tabBindings(profiles({ all, playstation: none }), "playstation").inherited, false);
});

test("the first change in a type copies All into its own profile", () => {
  const all = { ...none, saveClip: "Pad:Back+South", screenshot: "Pad:Misc" };
  const next = editProfile(profiles({ all }), "nintendo", { screenshot: "Pad:Guide" });
  assert.deepEqual(next.nintendo, { ...all, screenshot: "Pad:Guide" });
  // All itself stays, and so do the other types.
  assert.deepEqual(next.all, all);
  assert.equal(next.xbox, null);
  // From then on edits go to the type's own.
  const again = editProfile({ ...next, all: { ...all, saveClip: "" } }, "nintendo", { record: "Pad:Start" });
  assert.equal(again.nintendo?.saveClip, "Pad:Back+South");
  assert.equal(again.nintendo?.record, "Pad:Start");
  // Changing All touches All only.
  assert.equal(editProfile(next, "all", { console: "Pad:LB" }).nintendo?.console, "");
});

test("a reset drops the type's own profile, back to All", () => {
  const own = profiles({ xbox: { ...none, screenshot: "Pad:Guide" } });
  const reset = resetProfile(own, "xbox");
  assert.equal(reset.xbox, null);
  assert.equal(tabBindings(reset, "xbox").inherited, true);
});

test("a binding is placed against the bindings of its own profile only", () => {
  const xbox = { ...none, saveClip: "Pad:Guide" };
  // Guide is tapped in Xbox's profile: recorded for the screenshot it is held.
  assert.deepEqual(placeBinding(xbox, "screenshot", "Pad:Xbox"), { screenshot: "Hold:Pad:Xbox" });
  // In another profile that has nothing on Guide it stays tapped.
  assert.deepEqual(placeBinding(none, "screenshot", "Pad:PS"), { screenshot: "Pad:PS" });
});

test("recording in a tab needs a pad that tab listens to", () => {
  assert.equal(canRecord("all", []), false);
  assert.equal(canRecord("all", ["other"]), true);
  assert.equal(canRecord("playstation", ["xbox"]), false);
  assert.equal(canRecord("playstation", ["xbox", "playstation"]), true);
});

test("what the app names follows the pad last used, else the keyboard", () => {
  const config = {
    saveClipHotkey: "Ctrl+Shift+S",
    saveClipHotkeyAlt: "",
    toggleBufferHotkey: "",
    toggleBufferHotkeyAlt: "",
    screenshotHotkey: "",
    screenshotHotkeyAlt: "Mouse5",
    recordHotkey: "",
    recordHotkeyAlt: "",
    consoleHotkey: "Alt+C",
    consoleHotkeyAlt: "",
    padProfiles: profiles({
      all: { ...none, saveClip: "Pad:Back+South", record: "Pad:Start" },
      nintendo: { ...none, saveClip: "Pad:Misc" },
    }),
    padLastUsed: null,
  } satisfies Partial<AppConfig>;
  // No pad used yet: the keyboard's, the second when the first is empty.
  assert.deepEqual(shownHotkey(config, "saveClip"), { value: "Ctrl+Shift+S", family: "xbox" });
  assert.equal(shownHotkey(config, "screenshot").value, "Mouse5");
  // Nothing on the keyboard: the pad's anyway.
  assert.equal(shownHotkey(config, "record").value, "Pad:Start");
  assert.equal(shownHotkey(config, "toggleBuffer").value, "");
  // A PlayStation pad, going by All.
  const ps = { ...config, padLastUsed: "playstation" as const };
  assert.deepEqual(shownHotkey(ps, "saveClip"), { value: "Pad:Back+South", family: "playstation" });
  assert.equal(hotkeyLabel(shownHotkey(ps, "saveClip").value, "playstation"), "Share + Cross");
  // A Switch pad, going by Nintendo's own — which leaves the console unbound.
  const switchPad = { ...config, padLastUsed: "nintendo" as const };
  assert.equal(shownHotkey(switchPad, "saveClip").value, "Pad:Misc");
  assert.equal(shownHotkey(switchPad, "console").value, "Alt+C");
});
