import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Segmented, Toggle } from "@/components/ui/Controls";
import { Button } from "@/components/ui/Button";
import { DISCORD_LIMITS } from "@/lib/discord";
import { useStreamer } from "@/lib/streamer";
import { useFriends } from "@/lib/friends";
import { toast } from "@/lib/toastBus";
import { useWebhook, webhookApi } from "@/lib/webhook";
import type { DiscordLimit, StreamerMode } from "@/lib/types";
import { Row, USAGE_HINT } from "./shared";

export function BehaviourTab() {
  // `useShallow`, siehe `SourceTrouble`.
  const { config, patchConfig } = useEngine(
    useShallow((s) => ({ config: s.config, patchConfig: s.patchConfig })),
  );
  const streaming = useStreamer((s) => s.active);
  const streamerMode = config.streamerMode ?? "off";
  const signedIn = useFriends((s) => s.signedIn);
  const webhook = useWebhook();
  const act = (call: Promise<void>) => void call.catch((err) => toast("error", String(err)));

  return (
    <div className="space-y-8">
      <section>
        <SectionTitle title="Behaviour" />
        <Card className="divide-y divide-line">
          <Row label="Start the buffer automatically">
            <Toggle
              checked={config.buffer.autoStart}
              onChange={(autoStart) =>
                patchConfig({ buffer: { ...config.buffer, autoStart } })
              }
            />
          </Row>
          <Row
            label="Only buffer in game"
            hint={
              config.buffer.autoStart
                ? "Starts once a game is in the foreground, stops half a minute after it exits"
                : "Needs automatic start"
            }
          >
            <Toggle
              checked={config.onlyBufferInGame}
              disabled={!config.buffer.autoStart}
              onChange={(onlyBufferInGame) => patchConfig({ onlyBufferInGame })}
            />
          </Row>
          <Row label="Start with Windows" hint="Starts hidden in the tray">
            <Toggle
              checked={config.autoStartWithWindows}
              onChange={(autoStartWithWindows) =>
                patchConfig({ autoStartWithWindows })
              }
            />
          </Row>
          <Row label="Send anonymous usage stats" hint={USAGE_HINT}>
            <Toggle
              checked={config.usageStats && config.usageStatsAsked}
              onChange={(usageStats) => patchConfig({ usageStats, usageStatsAsked: true })}
            />
          </Row>
        </Card>
      </section>
      <section>
        <SectionTitle title="Streaming" />
        <Card className="divide-y divide-line">
          <Row
            label="Streamer mode"
            hint={streamerMode === "automatic" && streaming ? "Active" : undefined}
          >
            <Segmented<StreamerMode>
              value={streamerMode}
              options={[
                { key: "off", label: "Off" },
                { key: "on", label: "On" },
                { key: "automatic", label: <span title="While OBS or Streamlabs runs">Automatic</span> },
              ]}
              onChange={(streamerMode) => patchConfig({ streamerMode })}
            />
          </Row>
        </Card>
      </section>
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
    </div>
  );
}
