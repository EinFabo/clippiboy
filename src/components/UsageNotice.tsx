import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";

/**
 * Asks once whether the anonymous usage stats may go out: someone who skipped
 * the setup, or who updated from a version that only told them. It stays at
 * the top of the page until "Allow" or "No thanks" — a notice that went by on
 * its own was no consent. Until then the core counts and sends nothing.
 */
export function UsageNotice() {
  const { config, patchConfig } = useEngine(
    useShallow((s) => ({ config: s.config, patchConfig: s.patchConfig })),
  );
  if (!config.setupDone || !config.usageStats || config.usageStatsAsked) return null;

  const choose = (usageStats: boolean) => void patchConfig({ usageStats, usageStatsAsked: true });

  return (
    <Card className="mb-6 flex items-center gap-4 border-accent/40 bg-accent/10 p-4">
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium text-ink">Send anonymous usage stats?</p>
        <p className="mt-0.5 text-xs text-ink-muted">
          Once a day: the version, how many clips, which features. No content, no account, your IP
          is not stored. You can change it in Settings → Behaviour.
        </p>
      </div>
      <Button size="sm" variant="ghost" onClick={() => choose(false)}>
        No thanks
      </Button>
      <Button size="sm" variant="primary" onClick={() => choose(true)}>
        Allow
      </Button>
    </Card>
  );
}
