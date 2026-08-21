import { useEngine } from "@/store";
import { Card } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { cn } from "@/lib/cn";

/**
 * Was an den Tonquellen gerade nicht stimmt — dort gezeigt, wo aufgenommen
 * wird, nicht nur im Mixer.
 *
 * Eine Quelle, die nicht startet, wird trotzdem als Spur mitgeschrieben: Der
 * Clip bekommt sie dann als Stille. Das fällt erst nach dem Spielen auf, wenn
 * die Aufnahme längst gelaufen ist — deshalb muss es vorher jemand sehen.
 */
export function SourceTrouble({
  onOpenMixer,
}: {
  onOpenMixer: () => void;
}) {
  const { config, sourceErrors, sourceWarnings } = useEngine();

  const broken = config.sources.filter(
    (s) => s.enabled && sourceErrors[s.id],
  );
  const odd = config.sources.filter(
    (s) => s.enabled && !sourceErrors[s.id] && sourceWarnings[s.id],
  );
  if (broken.length === 0 && odd.length === 0) return null;

  const severe = broken.length > 0;
  return (
    <Card
      className={cn(
        "flex items-start gap-4 p-4",
        severe ? "border-live/40 bg-live/10" : "border-warn/40 bg-warn/10",
      )}
    >
      <div className="min-w-0 flex-1 space-y-1.5">
        {broken.map((source) => (
          <p key={source.id} className="text-[13px] leading-relaxed text-ink-muted">
            <span className="font-medium text-live">{source.label}</span>{" "}
            nimmt nicht auf — die Spur bliebe im Clip stumm.{" "}
            <span className="text-ink-faint">{sourceErrors[source.id]}</span>
          </p>
        ))}
        {odd.map((source) => (
          <p key={source.id} className="text-[13px] leading-relaxed text-ink-muted">
            <span className="font-medium text-warn">{source.label}</span>{" "}
            <span className="text-ink-faint">{sourceWarnings[source.id]}</span>
          </p>
        ))}
      </div>
      <Button size="sm" className="shrink-0" onClick={onOpenMixer}>
        Zum Mixer
      </Button>
    </Card>
  );
}
