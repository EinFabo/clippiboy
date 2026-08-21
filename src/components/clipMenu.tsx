import {
  IconArrowUpRight,
  IconCopy,
  IconFolder,
  IconHeart,
  IconPaste,
  IconPencil,
  IconPlay,
  IconTrash,
} from "@/components/icons";
import { useMenu, type MenuEntry, type MenuTrigger } from "@/components/ui/Menu";
import { useEngine } from "@/store";
import { api, inTauri } from "@/lib/ipc";
import type { Clip } from "@/lib/types";

/** Was die jeweilige Stelle zusätzlich beisteuert. */
interface Options {
  /** Nur in der Galerie: Der Player ist ja noch zu. */
  onOpen?: () => void;
  /** Nur in der Galerie: Der Editor hat sein eigenes Namensfeld. */
  onRename?: () => void;
  onDelete: () => void;
}

const icon = "h-4 w-4";

/**
 * Das Rechtsklick-Menü eines Clips — dieselben Einträge in der Galerie wie im
 * Player, nur dass dort „Öffnen" und „Umbenennen" fehlen.
 */
export function useClipMenu() {
  const menu = useMenu();
  const setFavorite = useEngine((state) => state.setFavorite);
  const fileClip = useEngine((state) => state.fileClip);

  return (trigger: MenuTrigger, clip: Clip, options: Options) => {
    const entries: MenuEntry[] = [];

    if (options.onOpen) {
      entries.push({
        kind: "item",
        label: "Öffnen",
        icon: <IconPlay className={icon} />,
        onSelect: options.onOpen,
      });
    }
    entries.push({
      kind: "item",
      label: "Mit Standardplayer öffnen",
      icon: <IconArrowUpRight className={icon} />,
      disabled: !inTauri,
      onSelect: () => void api.openClip(clip.id),
    });
    if (options.onRename) {
      entries.push({
        kind: "item",
        label: "Umbenennen",
        icon: <IconPencil className={icon} />,
        onSelect: options.onRename,
      });
    }
    entries.push({
      kind: "item",
      label: clip.favorite ? "Herz wegnehmen" : "Als Favorit merken",
      icon: <IconHeart filled={clip.favorite} className={icon} />,
      onSelect: async () => {
        await setFavorite(clip.id, !clip.favorite);
        // In der Galerie darf die Datei sofort umziehen. Im Player hält das
        // Videoelement sie noch — dort holt es `closePlayer` nach, und ein
        // zweiter Anlauf hier schadet nicht: Liegt sie schon richtig, tut
        // `fileClip` nichts.
        if (options.onOpen) await fileClip(clip.id);
      },
    });

    entries.push({ kind: "separator" });
    entries.push({
      kind: "item",
      label: "Clip kopieren",
      // Der eine Eintrag, den es sonst nirgends gibt: die Datei selbst in der
      // Zwischenablage, fertig für Strg+V in Discord.
      icon: <IconCopy className={icon} />,
      shortcut: "für Discord",
      disabled: !inTauri,
      onSelect: () => void api.copyClipFile(clip.id),
    });
    entries.push({
      kind: "item",
      label: "Pfad kopieren",
      icon: <IconPaste className={icon} />,
      disabled: !inTauri,
      onSelect: () => void api.clipboardWriteText(clip.path),
    });
    entries.push({
      kind: "item",
      label: "Im Ordner zeigen",
      icon: <IconFolder className={icon} />,
      disabled: !inTauri,
      onSelect: () => void api.revealClip(clip.id),
    });

    entries.push({ kind: "separator" });
    entries.push({
      kind: "item",
      label: "Löschen",
      icon: <IconTrash className={icon} />,
      danger: true,
      onSelect: options.onDelete,
    });

    menu.open(trigger, entries);
  };
}
