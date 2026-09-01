import {
  IconArrowUpRight,
  IconCamera,
  IconCopy,
  IconExport,
  IconFolder,
  IconHeart,
  IconPaste,
  IconPencil,
  IconPlay,
  IconScissors,
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
  /** Write a smaller copy to send. Optional: it opens a dialog at the call site. */
  onExport?: () => void;
  /**
   * Throw the untouched recording away. Optional because it has to ask first,
   * and the asking belongs where the menu was opened from — the player has the
   * button in its editor pane already and passes nothing.
   */
  onDiscardOriginal?: () => void;
}

const icon = "h-4 w-4";

/**
 * A clip's right-click menu — the same entries in the gallery as in the player,
 * except that "Open" and "Rename" are missing there.
 *
 * A screenshot gets the same menu, worded for a picture, plus the one entry a
 * still has and a recording does not: the picture itself on the clipboard.
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
        icon: clip.screenshot ? (
          <IconCamera className={icon} />
        ) : (
          <IconPlay className={icon} />
        ),
        onSelect: options.onOpen,
      });
    }
    entries.push({
      kind: "item",
      label: clip.screenshot ? "Open in default viewer" : "Open in default player",
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
    // The picture first: on a screenshot that is the one you actually want —
    // in a chat window a pasted file is an attachment, a pasted picture is a
    // picture.
    if (clip.screenshot) {
      entries.push({
        kind: "item",
        label: "Copy picture",
        icon: <IconCamera className={icon} />,
        disabled: !inTauri,
        onSelect: () => void api.copyClipImage(clip.id),
      });
    }
    entries.push({
      kind: "item",
      label: clip.screenshot ? "Copy file" : "Copy clip",
      // The one entry you find nowhere else: the file itself on the clipboard,
      // ready for Ctrl+V in Discord.
      icon: <IconCopy className={icon} />,
      disabled: !inTauri,
      onSelect: () => void api.copyClipFile(clip.id),
    });
    // Recordings only: a still is small already, and the arithmetic here is all
    // about length.
    if (!clip.screenshot && options.onExport) {
      const onExport = options.onExport;
      entries.push({
        kind: "item",
        label: "Export a copy…",
        icon: <IconExport className={icon} />,
        disabled: !inTauri,
        onSelect: onExport,
      });
    }
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

    // Only worth offering while there is something to release, and only where
    // the calling site can ask first — throwing the recording away cannot be
    // undone. The record on the clip outlives the file, so `originalAvailable`
    // is the question, not `original`; see the note on the type.
    if (clip.originalAvailable && options.onDiscardOriginal) {
      const onDiscardOriginal = options.onDiscardOriginal;
      entries.push({ kind: "separator" });
      entries.push({
        kind: "item",
        label: "Free up space…",
        icon: <IconScissors className={icon} />,
        disabled: !inTauri,
        onSelect: onDiscardOriginal,
      });
    }

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
