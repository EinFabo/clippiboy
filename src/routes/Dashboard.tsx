import { useState } from "react";
import { useEngine } from "@/store";
import { Card, Pill, SectionTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { ClipPlayer } from "@/components/ClipPlayer";
import { IconArrowUpRight, IconScissors } from "@/components/icons";
import { clipName, formatAgo, formatBufferSeconds, formatDuration } from "@/lib/format";
import { fileUrl } from "@/lib/ipc";
import type { Route } from "@/components/NavBar";
import { SourceTrouble } from "@/components/SourceTrouble";
import { cn } from "@/lib/cn";

export function Dashboard({ onNavigate }: { onNavigate: (r: Route) => void }) {
  const {
    config,
    clips,
    bufferActive,
    bufferedSeconds,
    detectedGame,
    toggleBuffer,
    saveClip,
    deleteClip,
  } = useEngine();
  const [playing, setPlaying] = useState<number | null>(null);
  const recent = clips.slice(0, 3);

  return (
    <div className="space-y-12">
      <header className="pt-10 pb-4 text-center">
        <h1 className="display text-5xl">ClippiBoy</h1>
        <p className="mx-auto mt-4 max-w-md text-[15px] leading-relaxed text-white/70">
          Läuft im Hintergrund, hört auf jede Audioquelle einzeln, und speichert
          rückwirkend genau den Moment, der es wert war.
        </p>
        <div className="mt-7 flex justify-center gap-3">
          <Button
            variant={bufferActive ? "secondary" : "primary"}
            onClick={toggleBuffer}
          >
            {bufferActive ? "Puffer stoppen" : "Puffer starten"}
          </Button>
          <Button
            variant="secondary"
            icon={<IconScissors className="h-4 w-4" />}
            onClick={saveClip}
            disabled={!bufferActive}
          >
            Clip speichern
          </Button>
        </div>
      </header>

      <section className="grid grid-cols-4 gap-4">
        <Stat
          label="Replay-Puffer"
          value={
            bufferActive
              ? formatBufferSeconds(Math.round(bufferedSeconds))
              : "aus"
          }
          hint={`von ${formatBufferSeconds(config.buffer.seconds)}`}
          live={bufferActive}
        />
        <Stat
          label="Erkanntes Spiel"
          value={detectedGame ?? "keins"}
          hint={
            detectedGame
              ? "Trägt den Namen an den nächsten Clip"
              : "Kein Spiel im Vordergrund"
          }
        />
        <Stat
          label="Aufnahme"
          value={`${config.recording.height}p${config.recording.fps}`}
          hint={`${Math.round(config.recording.bitrateKbps / 1000)} Mbit/s · ${config.recording.encoder.toUpperCase()}`}
          onClick={() => onNavigate("recording")}
        />
        <Stat
          label="Audioquellen"
          value={String(config.sources.filter((s) => s.enabled).length)}
          hint={`${config.sources.filter((s) => s.separateTrack).length} eigene Spuren`}
          onClick={() => onNavigate("audio")}
        />
      </section>

      <SourceTrouble onOpenMixer={() => onNavigate("audio")} />

      <section>
        <SectionTitle
          title="Neueste Clips"
          action={
            <Button size="sm" variant="ghost" onClick={() => onNavigate("clips")}>
              Alle ansehen
            </Button>
          }
        />
        {recent.length === 0 ? (
          <Card className="grid h-44 place-items-center text-sm text-ink-muted">
            Noch keine Clips — starte den Puffer und drücke{" "}
            <kbd className="mx-1 rounded-inner border border-line bg-elevated px-2 py-0.5 text-xs">
              {config.saveClipHotkey}
            </kbd>
          </Card>
        ) : (
          <div className="grid grid-cols-3 gap-4">
            {recent.map((clip, index) => (
              <Card
                key={clip.id}
                interactive
                onClick={() => setPlaying(index)}
                className="cursor-pointer overflow-hidden"
              >
                <div className="relative aspect-video bg-gradient-to-br from-accent-deep/40 to-black">
                  {clip.thumbPath && (
                    <img
                      src={fileUrl(clip.thumbPath)}
                      alt=""
                      className="h-full w-full object-cover"
                    />
                  )}
                  {/* Eine Zeile für beide Marken: Ein langer Spielname kürzt
                      sich, statt sich unter die Dauer zu schieben. */}
                  <div className="absolute inset-x-3 bottom-3 flex items-end justify-between gap-2">
                    <Pill className="min-w-0">
                      <span className="min-w-0 truncate">{clip.game ?? "Unbekannt"}</span>
                    </Pill>
                    <Pill className="shrink-0">{formatDuration(clip.durationMs)}</Pill>
                  </div>
                </div>
                <div className="flex items-center justify-between p-4">
                  <div className="min-w-0">
                    <p className="truncate text-sm font-medium">
                      {clipName(clip)}
                    </p>
                    <p className="mt-1 text-xs text-ink-muted">
                      {formatAgo(clip.createdAt)}
                    </p>
                  </div>
                  <span className="grid h-8 w-8 shrink-0 place-items-center rounded-pill border border-line text-ink-muted">
                    <IconArrowUpRight className="h-4 w-4" />
                  </span>
                </div>
              </Card>
            ))}
          </div>
        )}
      </section>

      {playing !== null && recent.length > 0 && (
        <ClipPlayer
          clips={recent}
          index={Math.min(playing, recent.length - 1)}
          onIndexChange={setPlaying}
          onClose={() => setPlaying(null)}
          onDelete={deleteClip}
          onOpenMixer={() => onNavigate("audio")}
        />
      )}
    </div>
  );
}

function Stat({
  label,
  value,
  hint,
  live,
  onClick,
}: {
  label: string;
  value: string;
  hint: string;
  live?: boolean;
  onClick?: () => void;
}) {
  return (
    <Card
      interactive={!!onClick}
      onClick={onClick}
      className={cn("p-5", onClick && "cursor-pointer")}
    >
      <div className="flex items-center gap-2 text-xs font-medium text-ink-muted">
        {live && <span className="h-2 w-2 animate-pulse rounded-pill bg-live" />}
        {label}
      </div>
      <p className="display mt-3 truncate text-3xl">{value}</p>
      <p className="mt-1 text-xs text-ink-faint">{hint}</p>
    </Card>
  );
}
