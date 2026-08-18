import { useMemo, useState } from "react";
import { useEngine } from "@/store";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Meter, Slider, Toggle } from "@/components/ui/Controls";
import {
  IconApp,
  IconMic,
  IconPlus,
  IconSpeaker,
  IconTrash,
} from "@/components/icons";
import { cn } from "@/lib/cn";
import type { AudioSource, SourceKind } from "@/lib/types";

function sourceIcon(kind: SourceKind) {
  if (kind.type === "inputDevice") return <IconMic className="h-4 w-4" />;
  if (kind.type === "outputDevice") return <IconSpeaker className="h-4 w-4" />;
  return <IconApp className="h-4 w-4" />;
}

function sourceHint(kind: SourceKind, deviceName: (id: string) => string) {
  switch (kind.type) {
    case "inputDevice":
      return `Eingang · ${deviceName(kind.deviceId)}`;
    case "outputDevice":
      return `Ausgang (Loopback) · ${deviceName(kind.deviceId)}`;
    case "process":
      return kind.mode === "include"
        ? `Anwendung · PID ${kind.pid}`
        : `Alles außer PID ${kind.pid}`;
  }
}

export function AudioMixer() {
  const {
    config,
    devices,
    processes,
    levels,
    sourceErrors,
    upsertSource,
    removeSource,
  } = useEngine();
  const [adding, setAdding] = useState(false);

  const deviceName = useMemo(
    () => (id: string) => devices.find((d) => d.id === id)?.name ?? id,
    [devices],
  );

  const anySolo = config.sources.some((s) => s.solo);

  return (
    <div className="space-y-8">
      <header className="pt-10">
        <h1 className="display text-4xl">Audio-Mixer</h1>
        <p className="mt-3 max-w-lg text-[15px] leading-relaxed text-white/70">
          Beliebig viele Quellen gleichzeitig: Ausgabegeräte, einzelne
          Anwendungen und Mikrofone. Jede Quelle kann in den Hauptmix oder auf
          eine eigene Tonspur im Clip laufen.
        </p>
      </header>

      <section>
        <SectionTitle
          title="Quellen"
          action={
            <Button
              size="sm"
              variant="primary"
              icon={<IconPlus className="h-4 w-4" />}
              onClick={() => setAdding((v) => !v)}
            >
              Quelle hinzufügen
            </Button>
          }
        />

        {adding && (
          <AddSourcePanel
            onClose={() => setAdding(false)}
            onAdd={(s) => {
              upsertSource(s);
              setAdding(false);
            }}
          />
        )}

        <div className="space-y-3">
          {config.sources.map((source) => {
            const dimmed = anySolo && !source.solo;
            const level = levels[source.id] ?? 0;
            return (
              <Card
                key={source.id}
                className={cn(
                  "p-4 transition-opacity duration-200",
                  (!source.enabled || dimmed) && "opacity-45",
                )}
              >
                <div className="flex items-center gap-4">
                  <span className="grid h-9 w-9 shrink-0 place-items-center rounded-pill bg-elevated text-ink-muted">
                    {sourceIcon(source.kind)}
                  </span>

                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <input
                        value={source.label}
                        onChange={(e) =>
                          upsertSource({ ...source, label: e.target.value })
                        }
                        className="w-44 truncate rounded-inner bg-transparent text-sm font-medium
                          outline-none hover:bg-elevated focus:bg-elevated px-1.5 py-0.5"
                      />
                      {source.separateTrack && (
                        <span className="rounded-pill bg-accent/15 px-2 py-0.5 text-[11px] font-medium text-accent-bright">
                          eigene Spur
                        </span>
                      )}
                    </div>
                    <p
                      className={cn(
                        "mt-0.5 truncate px-1.5 text-xs",
                        sourceErrors[source.id] ? "text-live" : "text-ink-faint",
                      )}
                    >
                      {sourceErrors[source.id] ?? sourceHint(source.kind, deviceName)}
                    </p>
                    <div className="mt-2.5 px-1.5">
                      <Meter level={source.muted ? 0 : level} />
                    </div>
                  </div>

                  <div className="flex w-52 shrink-0 items-center gap-3">
                    <Slider
                      label={`Lautstärke ${source.label}`}
                      value={source.gainDb}
                      min={-30}
                      max={12}
                      step={0.5}
                      onChange={(gainDb) => upsertSource({ ...source, gainDb })}
                    />
                    <span className="w-14 shrink-0 text-right font-mono text-xs text-ink-muted">
                      {source.gainDb > 0 ? "+" : ""}
                      {source.gainDb.toFixed(1)} dB
                    </span>
                  </div>

                  <div className="flex shrink-0 items-center gap-1.5">
                    <MiniToggle
                      active={source.muted}
                      activeClass="bg-live/20 text-live"
                      onClick={() =>
                        upsertSource({ ...source, muted: !source.muted })
                      }
                    >
                      M
                    </MiniToggle>
                    <MiniToggle
                      active={source.solo}
                      activeClass="bg-accent/25 text-accent-bright"
                      onClick={() =>
                        upsertSource({ ...source, solo: !source.solo })
                      }
                    >
                      S
                    </MiniToggle>
                    <MiniToggle
                      active={source.separateTrack}
                      activeClass="bg-accent/25 text-accent-bright"
                      title="Eigene Tonspur im MP4"
                      onClick={() =>
                        upsertSource({
                          ...source,
                          separateTrack: !source.separateTrack,
                        })
                      }
                    >
                      ⧉
                    </MiniToggle>
                    <span className="mx-1">
                      <Toggle
                        label={`${source.label} aktiv`}
                        checked={source.enabled}
                        onChange={(enabled) =>
                          upsertSource({ ...source, enabled })
                        }
                      />
                    </span>
                    <button
                      aria-label="Quelle entfernen"
                      onClick={() => removeSource(source.id)}
                      className="grid h-8 w-8 place-items-center rounded-pill text-ink-faint
                        transition-colors hover:bg-live/15 hover:text-live"
                    >
                      <IconTrash className="h-4 w-4" />
                    </button>
                  </div>
                </div>
              </Card>
            );
          })}
        </div>

        {config.sources.length === 0 && (
          <Card className="grid h-32 place-items-center text-sm text-ink-muted">
            Noch keine Audioquelle konfiguriert.
          </Card>
        )}
      </section>

      <p className="pb-4 text-xs leading-relaxed text-ink-faint">
        Anwendungs-Quellen nutzen Prozess-Loopback (Windows 10 Build 20348+).
        Auf älteren Systemen steht nur die Aufnahme kompletter Ausgabegeräte zur
        Verfügung.
      </p>

      <AvailableSources processes={processes} />
    </div>
  );
}

function MiniToggle({
  active,
  activeClass,
  onClick,
  children,
  title,
}: {
  active: boolean;
  activeClass: string;
  onClick: () => void;
  children: React.ReactNode;
  title?: string;
}) {
  return (
    <button
      title={title}
      onClick={onClick}
      className={cn(
        "h-8 w-8 rounded-pill text-xs font-semibold transition-colors duration-150",
        active ? activeClass : "bg-elevated text-ink-faint hover:text-ink",
      )}
    >
      {children}
    </button>
  );
}

function AvailableSources({
  processes,
}: {
  processes: ReturnType<typeof useEngine.getState>["processes"];
}) {
  if (processes.length === 0) return null;
  return (
    <p className="pb-10 text-xs text-ink-faint">
      Erkannte Anwendungen mit Audio: {processes.map((p) => p.name).join(" · ")}
    </p>
  );
}

function AddSourcePanel({
  onAdd,
  onClose,
}: {
  onAdd: (s: AudioSource) => void;
  onClose: () => void;
}) {
  const { devices, processes, refreshSources } = useEngine();
  const outputs = devices.filter((d) => d.kind === "output");
  const inputs = devices.filter((d) => d.kind === "input");

  const make = (label: string, kind: SourceKind): AudioSource => ({
    id: `src-${crypto.randomUUID().slice(0, 8)}`,
    label,
    kind,
    enabled: true,
    gainDb: 0,
    muted: false,
    solo: false,
    separateTrack: false,
  });

  return (
    <Card className="mb-4 p-5">
      <div className="mb-4 flex items-center justify-between">
        <h3 className="text-sm font-semibold">Quelle auswählen</h3>
        <div className="flex gap-2">
          <Button size="sm" variant="ghost" onClick={refreshSources}>
            Aktualisieren
          </Button>
          <Button size="sm" variant="ghost" onClick={onClose}>
            Schließen
          </Button>
        </div>
      </div>

      <div className="grid grid-cols-3 gap-6">
        <SourceColumn
          title="Anwendungen"
          icon={<IconApp className="h-4 w-4" />}
          entries={processes.map((p) => ({
            key: String(p.pid),
            label: p.name,
            sub: p.exe,
            onPick: () =>
              onAdd(make(p.name, { type: "process", pid: p.pid, mode: "include" })),
          }))}
        />
        <SourceColumn
          title="Ausgänge (Loopback)"
          icon={<IconSpeaker className="h-4 w-4" />}
          entries={outputs.map((d) => ({
            key: d.id,
            label: d.name,
            sub: d.isDefault ? "Standardgerät" : "",
            onPick: () =>
              onAdd(make(d.name, { type: "outputDevice", deviceId: d.id })),
          }))}
        />
        <SourceColumn
          title="Eingänge"
          icon={<IconMic className="h-4 w-4" />}
          entries={inputs.map((d) => ({
            key: d.id,
            label: d.name,
            sub: d.isDefault ? "Standardgerät" : "",
            onPick: () =>
              onAdd(make(d.name, { type: "inputDevice", deviceId: d.id })),
          }))}
        />
      </div>
    </Card>
  );
}

function SourceColumn({
  title,
  icon,
  entries,
}: {
  title: string;
  icon: React.ReactNode;
  entries: Array<{
    key: string;
    label: string;
    sub: string;
    onPick: () => void;
  }>;
}) {
  return (
    <div>
      <div className="mb-2 flex items-center gap-2 text-xs font-medium text-ink-muted">
        {icon}
        {title}
      </div>
      <div className="space-y-1">
        {entries.map((e) => (
          <button
            key={e.key}
            onClick={e.onPick}
            className="w-full rounded-inner px-3 py-2 text-left transition-colors hover:bg-elevated"
          >
            <p className="truncate text-sm">{e.label}</p>
            {e.sub && (
              <p className="truncate text-[11px] text-ink-faint">{e.sub}</p>
            )}
          </button>
        ))}
        {entries.length === 0 && (
          <p className="px-3 py-2 text-xs text-ink-faint">nichts gefunden</p>
        )}
      </div>
    </div>
  );
}
