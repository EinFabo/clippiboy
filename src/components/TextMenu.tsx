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
 * Einen Wert setzen, den React verwaltet.
 *
 * Direkt `el.value = …` bekäme React nicht mit — der Entwurf im Feld spränge
 * beim nächsten Render zurück. Deshalb über den Setter des Prototyps und ein
 * `input`-Ereignis, genau wie es ein Tastendruck auslöst.
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

/** Die Auswahl durch etwas anderes ersetzen und die Schreibmarke dahinter. */
function replaceSelection(el: TextField, start: number, end: number, text: string) {
  const value = el.value;
  el.focus();
  setValue(el, value.slice(0, start) + text + value.slice(end));
  const at = start + text.length;
  el.setSelectionRange(at, at);
}

const icon = "h-4 w-4";

/**
 * Zwei Dinge auf einmal, beide global:
 *
 * 1. Das eingebaute Menü des WebViews („Zurück", „Aktualisieren",
 *    „Untersuchen") wird überall unterdrückt — es gehört nicht in eine App.
 * 2. Textfelder bekommen dafür ein eigenes Menü im Stil des Programms, sonst
 *    nähme man ihnen mit dem WebView-Menü auch das Einfügen weg.
 *
 * Der Text kommt aus dem Kern statt aus `navigator.clipboard`: Lesen aus der
 * Zwischenablage fragt im WebView um Erlaubnis, und dieser Dialog gehört
 * ebenso wenig in eine App, die ohnehin nativ ist.
 */
export function TextMenu() {
  const menu = useMenu();

  useEffect(() => {
    const onContextMenu = (event: MouseEvent) => {
      // Hat eine Kachel oder der Player schon ihr Menü aufgemacht, ist hier
      // nichts mehr zu tun — React ist mit seinen Behandlern vorher dran.
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
          label: "Ausschneiden",
          icon: <IconScissors className={icon} />,
          shortcut: "Strg+X",
          disabled: !inTauri || !editable || !selected,
          onSelect: async () => {
            await api.clipboardWriteText(selected);
            replaceSelection(el, start, end, "");
          },
        },
        {
          kind: "item",
          label: "Kopieren",
          icon: <IconCopy className={icon} />,
          shortcut: "Strg+C",
          disabled: !inTauri || !selected,
          onSelect: () => void api.clipboardWriteText(selected),
        },
        {
          kind: "item",
          label: "Einfügen",
          icon: <IconPaste className={icon} />,
          shortcut: "Strg+V",
          disabled: !inTauri || !editable,
          onSelect: async () => {
            const text = await api.clipboardReadText();
            // Nichts Brauchbares in der Zwischenablage: dann bleibt das Feld,
            // wie es ist.
            if (text) replaceSelection(el, start, end, text);
          },
        },
        { kind: "separator" },
        {
          kind: "item",
          label: "Alles markieren",
          icon: <IconSelectAll className={icon} />,
          shortcut: "Strg+A",
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
