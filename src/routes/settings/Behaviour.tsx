import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Segmented, Toggle } from "@/components/ui/Controls";
import { DISCORD_LIMITS } from "@/lib/discord";
import type { DiscordLimit } from "@/lib/types";
import { Row, USAGE_HINT } from "./shared";

export function BehaviourTab() {
  // `useShallow`, siehe `SourceTrouble`.
  const { config, patchConfig } = useEngine(
    useShallow((s) => ({ config: s.config, patchConfig: s.patchConfig })),
  );

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
        </Card>
      </section>
    </div>
  );
}
