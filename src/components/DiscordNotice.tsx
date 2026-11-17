import { useEffect, useState } from "react";
import { useEngine } from "@/store";
import { api, events, inTauri } from "@/lib/ipc";
import { discordHint, hasDiscordSource, runningDiscord } from "@/lib/discordSource";
import { toast } from "@/lib/toastBus";
import type { AudioProcess } from "@/lib/types";
import { Card } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { discordSource } from "@/routes/AudioMixer";
import type { Route } from "./NavBar";

type Hint = { kind: "add"; discord: AudioProcess } | { kind: "turnDown" };

/**
 * Once, after the first clip: friends in Discord are what most often drowns
 * out a clip. Without a Discord source it offers one, with one it points to
 * where it is turned down. Only said once there was a Discord to speak of —
 * a clip without one leaves the note for a later clip.
 */
export function DiscordNotice({ onNavigate }: { onNavigate: (route: Route) => void }) {
  const [hint, setHint] = useState<Hint | null>(null);
  const sources = useEngine((s) => s.config.sources);
  const upsertSource = useEngine((s) => s.upsertSource);

  useEffect(() => {
    if (!inTauri) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    let deciding = false;
    events
      .onClipSaved(async (clip) => {
        const { config } = useEngine.getState();
        if (deciding || clip.screenshot || !config.setupDone || config.discordHintShown) return;
        deciding = true;
        try {
          // Read now, not at start: Discord is often opened after ClippiBoy.
          const processes = await api
            .listAudioProcesses()
            .catch(() => useEngine.getState().processes);
          const { config: now } = useEngine.getState();
          if (cancelled || now.discordHintShown) return;
          const kind = discordHint(now.sources, processes);
          const discord = runningDiscord(processes);
          // Not marked as said here: clips are saved in the game, with this
          // window hidden. The note stays until it is answered; a restart
          // before that brings it back with the next clip.
          if (kind === "add" && discord) setHint({ kind, discord });
          else if (kind === "turnDown") setHint({ kind });
        } finally {
          deciding = false;
        }
      })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  if (!hint) return null;

  const answered = () => {
    setHint(null);
    void useEngine.getState().patchConfig({ discordHintShown: true });
  };
  // Added by hand on the Audio page in the meantime: nothing left to offer.
  if (hint.kind === "add" && hasDiscordSource(sources)) return null;

  const add = async (discord: AudioProcess) => {
    answered();
    await upsertSource(discordSource(discord));
    toast("ok", "Discord is on its own track now");
  };

  return (
    <Card className="mb-6 flex items-center gap-4 border-accent/40 bg-accent/10 p-4">
      <p className="min-w-0 flex-1 text-sm font-medium text-ink">
        {hint.kind === "add"
          ? "Friends too loud in your clips? Put Discord on its own track."
          : "Discord too loud in your clips?"}
      </p>
      <Button size="sm" variant="ghost" onClick={answered}>
        Not now
      </Button>
      {hint.kind === "add" ? (
        <Button size="sm" variant="primary" onClick={() => void add(hint.discord)}>
          Add Discord
        </Button>
      ) : (
        <Button
          size="sm"
          variant="primary"
          onClick={() => {
            answered();
            onNavigate("audio");
          }}
        >
          Turn it down
        </Button>
      )}
    </Card>
  );
}
