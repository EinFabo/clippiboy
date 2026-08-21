import { useEffect } from "react";

import {
  IconCopy,
  IconPaste,
  IconScissors,
  IconSelectAll,
} from "@/components/icons";
import { useMenu, type MenuEntry } from "@/components/ui/Menu";
import { api, inTauri } from "@/lib/ipc";
import { isTextField, type TextField } from "@/lib/dom";

/**
 * Set a value that React manages.
 *
 * React would not notice a plain `el.value = …` — the draft in the field would
 * jump back on the next render. Hence the prototype's setter plus an `input`
 * event, exactly what a key press raises.
 */
function setValue(el: TextField, value: string) {
  const proto =
    el instanceof HTMLTextAreaElement
      ? HTMLTextAreaElement.prototype
      : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
  setter?.call(el, value);
  el.dispatchEvent(new Event("input", { bubbles: true }));
}

/** Replace the selection with something else and put the caret behind it. */
function replaceSelection(el: TextField, start: number, end: number, text: string) {
  const value = el.value;
  el.focus();
  setValue(el, value.slice(0, start) + text + value.slice(end));
  const at = start + text.length;
  el.setSelectionRange(at, at);
}

const icon = "h-4 w-4";

/**
 * Two things at once, both global:
 *
 * 1. The WebView's built-in menu ("Back", "Reload", "Inspect") is suppressed
 *    everywhere — it does not belong in an app.
 * 2. Text fields get a menu of their own in the program's style instead;
 *    otherwise taking away the WebView menu would take paste away with it.
 *
 * The text comes from the core rather than `navigator.clipboard`: reading the
 * clipboard asks for permission in the WebView, and that dialog belongs in a
 * native app just as little.
 */
export function TextMenu() {
  const menu = useMenu();

  useEffect(() => {
    const onContextMenu = (event: MouseEvent) => {
      // If a tile or the player has already opened its menu, there is nothing
      // left to do here — React gets its handlers in first.
      if (event.defaultPrevented) return;
      event.preventDefault();

      const el = event.target;
      if (!isTextField(el)) return;

      const start = el.selectionStart ?? 0;
      const end = el.selectionEnd ?? 0;
      const selected = el.value.slice(start, end);
      const editable = !el.readOnly;

      const entries: MenuEntry[] = [
        {
          kind: "item",
          label: "Cut",
          icon: <IconScissors className={icon} />,
          shortcut: "Ctrl+X",
          disabled: !inTauri || !editable || !selected,
          onSelect: async () => {
            await api.clipboardWriteText(selected);
            replaceSelection(el, start, end, "");
          },
        },
        {
          kind: "item",
          label: "Copy",
          icon: <IconCopy className={icon} />,
          shortcut: "Ctrl+C",
          disabled: !inTauri || !selected,
          onSelect: () => void api.clipboardWriteText(selected),
        },
        {
          kind: "item",
          label: "Paste",
          icon: <IconPaste className={icon} />,
          shortcut: "Ctrl+V",
          disabled: !inTauri || !editable,
          onSelect: async () => {
            const text = await api.clipboardReadText();
            // Nothing usable on the clipboard: then the field stays as it is.
            if (text) replaceSelection(el, start, end, text);
          },
        },
        { kind: "separator" },
        {
          kind: "item",
          label: "Select all",
          icon: <IconSelectAll className={icon} />,
          shortcut: "Ctrl+A",
          disabled: el.value.length === 0,
          onSelect: () => {
            el.focus();
            el.select();
          },
        },
      ];

      menu.open(event, entries);
    };

    document.addEventListener("contextmenu", onContextMenu);
    return () => document.removeEventListener("contextmenu", onContextMenu);
  }, [menu]);

  return null;
}
