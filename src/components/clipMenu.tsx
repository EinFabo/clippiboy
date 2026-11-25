import {
  IconArrowUpRight,
  IconCamera,
  IconCopy,
  IconExport,
  IconFolder,
  IconFriends,
  IconHeart,
  IconLink,
  IconPaste,
  IconPencil,
  IconPlay,
  IconScissors,
  IconShare,
  IconSend,
  IconTrash,
} from "@/components/icons";
import { Button } from "@/components/ui/Button";
import { useMenu, type MenuEntry, type MenuTrigger } from "@/components/ui/Menu";
import { useEngine } from "@/store";
import { api, inTauri } from "@/lib/ipc";
import { useShare } from "@/lib/share";
import { discordMb } from "@/lib/discord";
import { toast } from "@/lib/toastBus";
import { planExport } from "@/lib/exportPlan";
import { isDiscordBusy, setDiscordBusy } from "@/lib/discordCopies";
import { timeLeft, useLinks } from "@/lib/links";
import { linkReason, shareAsLinkLabel } from "@/lib/linkReason";
import { postLabel, useWebhook, webhookApi } from "@/lib/webhook";
import type { Clip } from "@/lib/types";

/** What "Copy for Discord" aims at, from the setting "Discord upload limit". */
export function discordTarget(): number {
  return discordMb(useEngine.getState().config.discordLimit);
}

/** Whether "Copy for Discord" can run for a clip: now, not yet (shrinking), or never (too long). */
export function discordCopyState(clip: Clip): "ok" | "busy" | "tooLong" {
  if (isDiscordBusy(clip.id)) return "busy";
  const mb = discordTarget();
  return planExport(clip, mb * 1024 * 1024).tooLong ? "tooLong" : "ok";
}

/** Shrinks a clip to the Discord limit and copies it; the shrinking shows as busy meanwhile. */
export async function copyForDiscord(clip: Clip): Promise<void> {
  if (isDiscordBusy(clip.id)) return;
  setDiscordBusy(clip.id, true);
  try {
    await run(api.exportForDiscord(clip.id, discordTarget()), "Copied · Ctrl+V in Discord");
  } finally {
    setDiscordBusy(clip.id, false);
  }
}

/** Awaits a menu action: its failure is toasted, its success says `done` if given. */
async function run(call: Promise<unknown>, done?: string) {
  try {
    await call;
    if (done) toast("ok", done);
  } catch (err) {
    toast("error", String(err));
  }
}

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
 * "Share as link", or for a clip that has one, "Copy link" and "Delete link".
 * Read from the store when the menu opens — a menu is a moment, it need not
 * follow the store while open.
 */
function linkEntries(clip: Clip): MenuEntry[] {
  const store = useLinks.getState();
  const { create, remove, refresh } = store;
  // What the store knows may be old (see `linkReason`): every opening asks
  // the server again for next time.
  const { live: link, reason, enabled } = linkReason(clip.id, store, Date.now(), inTauri);
  void refresh();
  if (link) {
    return [
      {
        kind: "item",
        label: `Copy share link · ${timeLeft(link)}`,
        icon: <IconLink className={icon} />,
        onSelect: () => void create(clip.id, clip.title ?? "Clip"),
      },
      {
        kind: "item",
        label: "Delete share link",
        icon: <IconTrash className={icon} />,
        onSelect: () => void remove(clip.id),
      },
    ];
  }
  // Why it cannot be had, right in the label: a menu has no room for a hint.
  return [
    {
      kind: "item",
      label: shareAsLinkLabel(reason),
      icon: <IconLink className={icon} />,
      disabled: !enabled,
      onSelect: () => void create(clip.id, clip.title ?? "Clip"),
    },
  ];
}

/**
 * The entries that put the clip on the clipboard — the top of the share menu,
 * and where the Overview takes its quickest copy action from.
 */
export function copyEntries(clip: Clip): MenuEntry[] {
  const entries: MenuEntry[] = [];
  // The picture first: on a screenshot that is the one you actually want —
  // in a chat window a pasted file is an attachment, a pasted picture is a
  // picture.
  if (clip.screenshot) {
    entries.push({
      kind: "item",
      label: "Copy picture",
      icon: <IconCamera className={icon} />,
      disabled: !inTauri,
      onSelect: () => void run(api.copyClipImage(clip.id), "Picture copied"),
    });
  }
  entries.push({
    kind: "item",
    label: clip.screenshot ? "Copy file" : "Copy clip",
    // The one entry you find nowhere else: the file itself on the clipboard,
    // ready for Ctrl+V in Discord.
    icon: <IconCopy className={icon} />,
    disabled: !inTauri,
    onSelect: () => void run(api.copyClipFile(clip.id), "Clip copied"),
  });
  if (!clip.screenshot) {
    const state = discordCopyState(clip);
    entries.push({
      kind: "item",
      label:
        state === "busy"
          ? "Copy for Discord · shrinking…"
          : state === "tooLong"
            ? `Copy for Discord · too long for ${discordTarget()} MB`
            : "Copy for Discord",
      shortcut: state === "ok" ? `Under ${discordTarget()} MB` : undefined,
      icon: <IconCopy className={icon} />,
      disabled: !inTauri || state !== "ok",
      onSelect: () => void copyForDiscord(clip),
    });
  }
  if (clip.screenshot) {
    entries.push({
      kind: "item",
      label: "Copy for Discord",
      shortcut: `Under ${discordTarget()} MB`,
      icon: <IconCopy className={icon} />,
      disabled: !inTauri,
      onSelect: () =>
        void run(api.exportShotForDiscord(clip.id, discordTarget()), "Copied · Ctrl+V in Discord"),
    });
  }
  return entries;
}

/**
 * The entries that send a clip somewhere — shared by the right-click menu and
 * the "Share" button in the player, so the two cannot drift apart. Read when a
 * menu opens, like {@link linkEntries}.
 */
export function shareEntries(
  clip: Clip,
  {
    onExport,
    onSend,
    dividerBeforeExport = false,
  }: {
    onExport?: () => void;
    onSend: (clip: Clip) => void;
    /** The Share button sets the export apart; the right-click menu does not. */
    dividerBeforeExport?: boolean;
  },
): MenuEntry[] {
  const entries: MenuEntry[] = copyEntries(clip);
  entries.push({
    kind: "item",
    label: "Send to a friend…",
    // Straight to their PC, full quality — see `share.rs`.
    icon: <IconFriends className={icon} />,
    disabled: !inTauri,
    onSelect: () => onSend(clip),
  });
  // A link for anyone, five days long — clips only, the page is a player.
  if (!clip.screenshot) entries.push(...linkEntries(clip));
  // The Discord channel connected in Settings → Sharing.
  const webhook = useWebhook.getState();
  if (webhook.connected) {
    entries.push({
      kind: "item",
      label: postLabel(webhook, clip.id),
      icon: <IconSend className={icon} />,
      disabled: !inTauri || webhook.posting.includes(clip.id),
      onSelect: () => void run(webhookApi.post(clip.id), `Posting to ${webhook.guild ?? "Discord"}…`),
    });
  }
  // Recordings only: a still is small already, and the arithmetic here is all
  // about length.
  if (!clip.screenshot && onExport) {
    if (dividerBeforeExport) entries.push({ kind: "separator" });
    entries.push({
      kind: "item",
      label: "Export a copy…",
      icon: <IconExport className={icon} />,
      disabled: !inTauri,
      onSelect: onExport,
    });
  }
  return entries;
}

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
  const pickFriend = useShare((state) => state.pick);

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
      onSelect: () => void run(api.openClip(clip.id)),
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
    entries.push(...shareEntries(clip, { onExport: options.onExport, onSend: pickFriend }));
    if (clip.recording && clip.facecam?.state === "failed") {
      entries.push({
        kind: "item",
        label: "Try adding the camera again",
        icon: <IconCamera className={icon} />,
        disabled: !inTauri,
        onSelect: () => void run(api.retryRecordingCam(clip.id)),
      });
    }
    entries.push({
      kind: "item",
      label: "Copy path",
      icon: <IconPaste className={icon} />,
      disabled: !inTauri,
      onSelect: () => void run(api.clipboardWriteText(clip.path), "Path copied"),
    });
    entries.push({
      kind: "item",
      label: "Show in folder",
      icon: <IconFolder className={icon} />,
      disabled: !inTauri,
      onSelect: () => void run(api.revealClip(clip.id)),
    });

    // Only worth offering while there is something to release, and only where
    // the calling site can ask first — throwing the recording away cannot be
    // undone. The record on the clip outlives the file, so `originalAvailable`
    // is the question, not `original`; see the note on the type.
    // A facecam clip's clean picture and cam are space to free as well: the
    // cam is baked in where the file shows it.
    const freeable = clip.originalAvailable || clip.facecam?.state === "ready";
    if (freeable && options.onDiscardOriginal) {
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

/**
 * A "Share" button that opens the clip's share entries in a glass menu under
 * it — the player, the screenshot viewer and the clip cards all use it. White
 * in the viewers; on a card it sits beside "Open" and matches it.
 */
export function ShareMenuButton({
  clip,
  onExport,
  size = "sm",
  variant = "primary",
  disabled,
  className,
}: {
  clip: Clip;
  /** Opens the export dialog; recordings only, see {@link shareEntries}. */
  onExport?: () => void;
  size?: "sm" | "md";
  variant?: "primary" | "secondary";
  disabled?: boolean;
  className?: string;
}) {
  const menu = useMenu();
  const pickFriend = useShare((state) => state.pick);
  return (
    <Button
      variant={variant}
      size={size}
      disabled={disabled}
      className={className}
      icon={<IconShare className="h-4 w-4" />}
      aria-haspopup="menu"
      onClick={(event) => {
        // Not the card or backdrop underneath: a click here is not "open" or "close".
        event.stopPropagation();
        const box = event.currentTarget.getBoundingClientRect();
        const entries = shareEntries(clip, {
          onExport,
          onSend: pickFriend,
          dividerBeforeExport: true,
        });
        menu.open(
          {
            clientX: box.right,
            clientY: box.bottom + 8,
            preventDefault: () => {},
            stopPropagation: () => {},
          },
          entries,
          { glass: true, alignRight: true, anchor: box, gap: 8 },
        );
      }}
    >
      Share
    </Button>
  );
}
