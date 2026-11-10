import type {
  AppConfig,
  HotkeyField,
  PadAction,
  PadBindings,
  PadProfileKey,
  PadProfiles,
  PadType,
  ProfileType,
} from "./types.ts";

/**
 * How a hotkey binding reads. The core stores one string per action:
 * `Ctrl+Shift+S`, `Mouse4`, `Ctrl+Mouse5`, `Pad:Back+South`, `Pad:Guide`, any
 * of them behind `Hold:` when the action fires on holding rather than tapping
 * (`src-tauri/src/bindings.rs`). Old bindings are plain keyboard combinations
 * and read as before; controller bindings written before 0.10 in Xbox names
 * (`Pad:View+A`) too.
 */

/**
 * Display name of a key. What gets stored is always the spelling the core's
 * shortcut parser understands — this only holds what is printed on the keyboard.
 * Keys whose name already matches are left out.
 */
const KEY_LABELS: Record<string, string> = {
  Super: "Win",
  Escape: "Esc",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  PageUp: "Page ↑",
  PageDown: "Page ↓",
  PrintScreen: "PrtSc",
  Delete: "Del",
  Insert: "Ins",
  CapsLock: "Caps",
  ScrollLock: "Scroll",
  NumLock: "Num",
  NumpadAdd: "Num +",
  NumpadSubtract: "Num −",
  NumpadMultiply: "Num ×",
  NumpadDivide: "Num ÷",
  NumpadDecimal: "Num .",
  NumpadEnter: "Num Enter",
  NumpadEqual: "Num =",
  Mouse3: "Mouse 3",
  Mouse4: "Mouse 4",
  Mouse5: "Mouse 5",
};

/** What is printed on the key. The numpad follows a pattern, the rest is in
    {@link KEY_LABELS}; everything else is already called what it is called. */
export function keyLabel(key: string): string {
  const numpad = /^Numpad([0-9])$/.exec(key);
  if (numpad) return `Num ${numpad[1]}`;
  return KEY_LABELS[key] ?? key;
}

const HOLD = /^hold:\s*/i;
const PAD = /^pad:/i;

/** Fires on holding, not tapping. */
export function isHold(value: string): boolean {
  return HOLD.test(value.trim());
}

/** The binding without `Hold:` — what is pressed. */
export function triggerOf(value: string): string {
  return value.trim().replace(HOLD, "");
}

/** The same thing pressed, tapped or held. An empty binding stays empty. */
export function withHold(value: string, hold: boolean): string {
  const trigger = triggerOf(value);
  if (!trigger) return "";
  return hold ? `Hold:${trigger}` : trigger;
}

/** A controller combo rather than keys or a mouse button. */
export function isPad(value: string): boolean {
  return PAD.test(triggerOf(value));
}

/** What is pressed, in one spelling, so that two bindings on the same input
    compare equal however they were written: `ctrl+s` and `Ctrl+S`,
    `Pad:View+A` and `Pad:Back+South`, `Shift+Ctrl+Mouse5` and
    `Ctrl+Shift+Mouse5`. Tap or hold plays no part. */
export function triggerKey(value: string): string {
  const trigger = triggerOf(value);
  if (!trigger) return "";
  if (isPad(value)) {
    const buttons = new Set(trigger.replace(PAD, "").split("+").map(padButton));
    return `pad:${[...buttons].sort().join("+").toLowerCase()}`;
  }
  const parts = trigger.split("+").map((part) => part.trim().toLowerCase());
  const key = (parts.pop() ?? "").replace(/^key(?=[a-z]$)/, "").replace(/^digit(?=[0-9]$)/, "");
  const mods = new Set(parts.map((mod) => (mod === "control" ? "ctrl" : mod)));
  return [...[...mods].sort(), key].join("+");
}

/** The changes that put the recorded binding `next` on `field`. One input may
    carry two actions, one tapped and one held: when another binding already
    sits on the same input in the same way and the other way is still free,
    `next` takes the other way. Both ways taken, it stays as it is — and the
    core reports the clash. So it does when the same action's other slot
    (`saveClipHotkey` / `saveClipHotkeyAlt`) is on that input: one action
    tapped and held on one input is no pair, only the tap firing late. */
export function placeBinding<F extends string>(
  bindings: Record<F, string>,
  field: F,
  next: string,
): Partial<Record<F, string>> {
  const key = triggerKey(next);
  if (!key) return { [field]: next } as Partial<Record<F, string>>;
  const others = (Object.keys(bindings) as F[]).filter(
    (other) => other !== field && triggerKey(bindings[other]) === key,
  );
  const sibling = field.endsWith("Alt") ? field.slice(0, -3) : `${field}Alt`;
  if (others.some((other) => other === sibling)) {
    return { [field]: next } as Partial<Record<F, string>>;
  }
  const taken = new Set(others.map((other) => isHold(bindings[other])));
  const hold = isHold(next);
  const placed = taken.has(hold) && !taken.has(!hold) ? withHold(next, !hold) : next;
  return { [field]: placed } as Partial<Record<F, string>>;
}

/** The changes that switch `field` to tapped or held. Shares it its input with
    another action that is already that way, the two swap: the other one takes
    the way `field` leaves. */
export function switchMode<F extends string>(
  bindings: Record<F, string>,
  field: F,
  hold: boolean,
): Partial<Record<F, string>> {
  const value = bindings[field];
  if (!triggerOf(value) || isHold(value) === hold) return {};
  const key = triggerKey(value);
  const partner = (Object.keys(bindings) as F[]).find(
    (other) =>
      other !== field && triggerKey(bindings[other]) === key && isHold(bindings[other]) === hold,
  );
  const changes = { [field]: withHold(value, hold) } as Partial<Record<F, string>>;
  if (partner) changes[partner] = withHold(bindings[partner], !hold);
  return changes;
}

/** Whose names a controller's buttons carry. */
export type PadFamily = "xbox" | "playstation" | "nintendo" | "gamecube";

/** Other spellings the core reads, by the position they stand for — the Xbox
    names older bindings were written in among them. Lower case. */
const PAD_ALIASES: Record<string, string> = {
  a: "South",
  b: "East",
  x: "West",
  y: "North",
  view: "Back",
  select: "Back",
  menu: "Start",
  options: "Start",
  xbox: "Guide",
  home: "Guide",
  ps: "Guide",
  share: "Misc",
  capture: "Misc",
  mic: "Misc",
  dpadup: "Up",
  dpaddown: "Down",
  dpadleft: "Left",
  dpadright: "Right",
};

/** What the core calls a controller button: its position (`South` is the
    bottom face button on every pad). */
export function padButton(name: string): string {
  return PAD_ALIASES[name.trim().toLowerCase()] ?? name.trim();
}

const PAD_LABELS: Record<PadFamily, Record<string, string>> = {
  xbox: {
    South: "A",
    East: "B",
    West: "X",
    North: "Y",
    Back: "View",
    Start: "Menu",
    Guide: "Xbox",
    Misc: "Share",
  },
  playstation: {
    South: "Cross",
    East: "Circle",
    West: "Square",
    North: "Triangle",
    Back: "Share",
    Start: "Options",
    Guide: "PS",
    Misc: "Mic",
    LB: "L1",
    RB: "R1",
    LT: "L2",
    RT: "R2",
    LS: "L3",
    RS: "R3",
  },
  nintendo: {
    South: "B",
    East: "A",
    West: "Y",
    North: "X",
    Back: "Minus",
    Start: "Plus",
    Guide: "Home",
    Misc: "Capture",
    LB: "L",
    RB: "R",
    LT: "ZL",
    RT: "ZR",
  },
  // Face buttons A (bottom), X (right), B (left), Y (top).
  gamecube: {
    South: "A",
    East: "X",
    West: "B",
    North: "Y",
    Start: "Start",
    LT: "L",
    RT: "R",
  },
};

/** What is printed on a controller button on a pad of `family`. */
export function padLabel(name: string, family: PadFamily = "xbox"): string {
  const button = padButton(name);
  return PAD_LABELS[family][button] ?? PAD_LABELS.xbox[button] ?? button;
}

/** The keys or buttons, each as it is printed — a controller's as on a pad
    of `family`. */
export function hotkeyKeys(value: string, family: PadFamily = "xbox"): string[] {
  const trigger = triggerOf(value);
  if (!trigger) return [];
  const keys = trigger.replace(PAD, "").split("+");
  return isPad(value) ? keys.map((key) => padLabel(key, family)) : keys.map(keyLabel);
}

/** The whole binding in one line: "Ctrl + Shift + S", "Hold Mouse 5". */
export function hotkeyLabel(value: string, family: PadFamily = "xbox"): string {
  const keys = hotkeyKeys(value, family).join(" + ");
  return isHold(value) && keys ? `Hold ${keys}` : keys;
}

// ---------------------------------------------------------------------------
// Controller profiles: keys and mouse buttons are bound once for everything
// (two slots per action); a controller binding lives in a profile — `all` for
// every pad, and per pad type one that replaces `all` on those pads entirely.

/** The actions, in the order the settings list them. */
export const PAD_ACTIONS: PadAction[] = ["saveClip", "toggleBuffer", "screenshot", "record", "console"];

/** The pad types that can have a profile, in tab order. */
export const PROFILE_TYPES: ProfileType[] = ["xbox", "playstation", "nintendo"];

/** What a profile's tab says. */
export const PROFILE_NAMES: Record<PadProfileKey, string> = {
  all: "All",
  xbox: "Xbox",
  playstation: "PlayStation",
  nintendo: "Nintendo",
};

/** The keyboard's two config fields for an action. */
const KEYBOARD_FIELDS: Record<PadAction, [HotkeyField, HotkeyField]> = {
  saveClip: ["saveClipHotkey", "saveClipHotkeyAlt"],
  toggleBuffer: ["toggleBufferHotkey", "toggleBufferHotkeyAlt"],
  screenshot: ["screenshotHotkey", "screenshotHotkeyAlt"],
  record: ["recordHotkey", "recordHotkeyAlt"],
  console: ["consoleHotkey", "consoleHotkeyAlt"],
};

/** Is anything bound in any profile? */
export function anyPadBinding(profiles: PadProfiles): boolean {
  return [profiles.all, ...PROFILE_TYPES.map((type) => profiles[type])].some(
    (set) => set !== null && PAD_ACTIONS.some((action) => triggerOf(set[action]) !== ""),
  );
}

/** Does the controller section show? Once a pad was ever seen (or is
    connected right now), or anything is bound to one — so it never hides
    bindings, and never shows for someone who has no controller. */
export function showsControllers(
  profiles: PadProfiles,
  seen: PadType[],
  connected: PadType[],
): boolean {
  return (
    seen.length > 0 ||
    connected.length > 0 ||
    anyPadBinding(profiles) ||
    PROFILE_TYPES.some((type) => profiles[type] !== null)
  );
}

/** The profile tabs: `all`, then each type seen, connected or with a profile
    of its own. `selector` is false — no tabs at all, the section edits `all`
    — while only one kind of pad is known and no type has a profile. */
export function profileTabs(
  profiles: PadProfiles,
  seen: PadType[],
  connected: PadType[],
): { tabs: PadProfileKey[]; selector: boolean } {
  const known = new Set<PadType>([...seen, ...connected]);
  const types = PROFILE_TYPES.filter((type) => known.has(type) || profiles[type] !== null);
  const selector = known.size >= 2 || PROFILE_TYPES.some((type) => profiles[type] !== null);
  return { tabs: selector ? ["all", ...types] : ["all"], selector };
}

/** The tab the section opens on: the profile the pad in use goes by — its
    type's when that has one, else `all`. */
export function openingTab(
  profiles: PadProfiles,
  inUse: PadType | null,
  tabs: PadProfileKey[],
): PadProfileKey {
  if (inUse && inUse !== "other" && profiles[inUse] !== null && tabs.includes(inUse)) return inUse;
  return "all";
}

/** What a tab shows: its own bindings, or — a type without a profile —
    `all`'s, borrowed. */
export function tabBindings(
  profiles: PadProfiles,
  tab: PadProfileKey,
): { bindings: PadBindings; inherited: boolean } {
  if (tab === "all") return { bindings: profiles.all, inherited: false };
  const own = profiles[tab];
  return own ? { bindings: own, inherited: false } : { bindings: profiles.all, inherited: true };
}

/** The profiles with `changes` made in `tab`. The first change in a type
    without a profile of its own copies `all` into it first: from then on that
    type goes by its own profile only. */
export function editProfile(
  profiles: PadProfiles,
  tab: PadProfileKey,
  changes: Partial<PadBindings>,
): PadProfiles {
  if (tab === "all") return { ...profiles, all: { ...profiles.all, ...changes } };
  const base = profiles[tab] ?? profiles.all;
  return { ...profiles, [tab]: { ...base, ...changes } };
}

/** The profiles without `type`'s own: it goes by `all` again. */
export function resetProfile(profiles: PadProfiles, type: ProfileType): PadProfiles {
  return { ...profiles, [type]: null };
}

/** Can a binding be recorded in `tab` right now? Only with a pad there to
    press on: any for `all`, one of the type for a type. */
export function canRecord(tab: PadProfileKey, connected: PadType[]): boolean {
  return tab === "all" ? connected.length > 0 : connected.includes(tab);
}

/** Whose names a pad of this type's buttons carry, as far as the type tells. */
export function familyOfType(type: PadType | null): PadFamily {
  return type === "playstation" || type === "nintendo" ? type : "xbox";
}

/** The binding the app names for `action` (dashboard, setup, console
    settings), with whose button names to say it in: the controller's from
    the profile of the pad last used, when it binds the action; else the
    keyboard's first or second; else the controller's anyway. Mirrors
    `AppConfig::hotkey_shown` in the core. */
export function shownHotkey(
  config: Pick<AppConfig, "padProfiles" | "padLastUsed" | HotkeyField>,
  action: PadAction,
): { value: string; family: PadFamily } {
  const used = config.padLastUsed;
  const profiles = config.padProfiles;
  const own = used && used !== "other" ? profiles[used] : null;
  const pad = (own ?? profiles.all)[action];
  const family = familyOfType(used);
  const [first, second] = KEYBOARD_FIELDS[action].map((field) => config[field]);
  if (used && triggerOf(pad)) return { value: pad, family };
  if (triggerOf(first)) return { value: first, family };
  if (triggerOf(second)) return { value: second, family };
  return { value: pad, family };
}
