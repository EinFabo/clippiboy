import { useEngine } from "@/store";
import { Card } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { cn } from "@/lib/cn";

/**
 * What is currently wrong with the audio sources — shown where recording
 * happens, not only in the mixer.
 *
 * A source that fails to start is still written as a track: the clip then gets
 * it as silence. That only shows up after playing, when the recording is long
 * done — which is why somebody has to see it beforehand.
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
            is not recording — the track would be silent in the clip.{" "}
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
        Open mixer
      </Button>
    </Card>
  );
}
