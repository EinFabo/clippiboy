import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Segmented, Toggle } from "@/components/ui/Controls";
import { Button } from "@/components/ui/Button";
import { DISCORD_LIMITS } from "@/lib/discord";
import { useFriends } from "@/lib/friends";
import { toast } from "@/lib/toastBus";
import { useWebhook, webhookApi } from "@/lib/webhook";
import type { DiscordLimit } from "@/lib/types";
import { Row } from "./shared";

export function SharingTab() {
  // `useShallow`, siehe `SourceTrouble`.
  const { config, patchConfig } = useEngine(
    useShallow((s) => ({ config: s.config, patchConfig: s.patchConfig })),
  );
  const signedIn = useFriends((s) => s.signedIn);
  const webhook = useWebhook();
  const act = (call: Promise<void>) => void call.catch((err) => toast("error", String(err)));

  return (
    <section>
      <SectionTitle title="Sharing" />
      <Card className="divide-y divide-line">
        <Row label="Discord upload limit">
          <Segmented<DiscordLimit>
            value={config.discordLimit ?? "free"}
            options={(Object.keys(DISCORD_LIMITS) as DiscordLimit[]).map((key) => ({
              key,
              label: `${DISCORD_LIMITS[key].label} ${DISCORD_LIMITS[key].mb} MB`,
            }))}
            onChange={(discordLimit) => patchConfig({ discordLimit })}
          />
        </Row>
        <Row label="Mute Discord voices when sharing">
          <Toggle
            checked={config.muteDiscordOnShare ?? false}
            onChange={(muteDiscordOnShare) => patchConfig({ muteDiscordOnShare })}
          />
        </Row>
        <Row
          label="Post clips to a Discord channel"
          hint={
            webhook.connected
              ? `Connected to ${webhook.guild ?? "a server"}`
              : webhook.connecting
                ? "Waiting for Discord…"
                : signedIn
                  ? undefined
                  : "Sign in on the Friends page first"
          }
        >
          {webhook.connected ? (
            <Button size="sm" variant="ghost" onClick={() => act(webhookApi.disconnect())}>
              Disconnect
            </Button>
          ) : webhook.connecting ? (
            <Button size="sm" variant="ghost" onClick={() => act(webhookApi.cancel())}>
              Cancel
            </Button>
          ) : (
            <Button size="sm" disabled={!signedIn} onClick={() => act(webhookApi.connect())}>
              Connect channel
            </Button>
          )}
        </Row>
        {webhook.connected && (
          <Row label="Post every clip automatically">
            <Toggle
              checked={config.webhookAutoPost ?? false}
              onChange={(webhookAutoPost) => patchConfig({ webhookAutoPost })}
            />
          </Row>
        )}
      </Card>
    </section>
  );
}
