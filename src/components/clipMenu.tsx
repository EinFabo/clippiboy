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

/** What the calling site contributes on top. */
interface Options {
  /** Gallery only: the player is still closed there. */
  onOpen?: () => void;
  /** Gallery only: the editor has a name field of its own. */
  onRename?: () => void;
  onDelete: () => void;
}

const icon = "h-4 w-4";

/**
 * A clip's right-click menu — the same entries in the gallery as in the player,
 * except that "Open" and "Rename" are missing there.
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
        label: "Open",
        icon: <IconPlay className={icon} />,
        onSelect: options.onOpen,
      });
    }
    entries.push({
      kind: "item",
      label: "Open in default player",
      icon: <IconArrowUpRight className={icon} />,
      disabled: !inTauri,
      onSelect: () => void api.openClip(clip.id),
    });
    if (options.onRename) {
      entries.push({
        kind: "item",
        label: "Rename",
        icon: <IconPencil className={icon} />,
        onSelect: options.onRename,
      });
    }
    entries.push({
      kind: "item",
      label: clip.favorite ? "Remove from favorites" : "Add to favorites",
      icon: <IconHeart filled={clip.favorite} className={icon} />,
      onSelect: async () => {
        await setFavorite(clip.id, !clip.favorite);
        // In the gallery the file may move right away. In the player the video
        // element still holds it — `closePlayer` catches up there, and a second
        // attempt here does no harm: if it already sits right, `fileClip` does
        // nothing.
        if (options.onOpen) await fileClip(clip.id);
      },
    });

    entries.push({ kind: "separator" });
    entries.push({
      kind: "item",
      label: "Copy clip",
      // The one entry you find nowhere else: the file itself on the clipboard,
      // ready for Ctrl+V in Discord.
      icon: <IconCopy className={icon} />,
      disabled: !inTauri,
      onSelect: () => void api.copyClipFile(clip.id),
    });
    entries.push({
      kind: "item",
      label: "Copy path",
      icon: <IconPaste className={icon} />,
      disabled: !inTauri,
      onSelect: () => void api.clipboardWriteText(clip.path),
    });
    entries.push({
      kind: "item",
      label: "Show in folder",
      icon: <IconFolder className={icon} />,
      disabled: !inTauri,
      onSelect: () => void api.revealClip(clip.id),
    });

    entries.push({ kind: "separator" });
    entries.push({
      kind: "item",
      // With the ellipsis, because it asks first — the deleting happens where
      // the menu was opened from.
      label: "Delete…",
      icon: <IconTrash className={icon} />,
      danger: true,
      onSelect: options.onDelete,
    });

    menu.open(trigger, entries);
  };
}
