import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Toggle } from "@/components/ui/Controls";
import { Row } from "./shared";

export function BehaviourTab() {
  // `useShallow`, siehe `SourceTrouble`.
  const { config, patchConfig } = useEngine(
    useShallow((s) => ({ config: s.config, patchConfig: s.patchConfig })),
  );

  return (
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
      </Card>
    </section>
  );
}
