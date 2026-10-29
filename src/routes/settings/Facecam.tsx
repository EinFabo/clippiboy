import { useEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { api, inTauri } from "@/lib/ipc";
import { Button } from "@/components/ui/Button";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Segmented, Select, Slider, Toggle } from "@/components/ui/Controls";
import { camStateLine, cornerOf, layoutAt, startCamPreview } from "@/lib/facecam";
import type { CamShape, FacecamConfig } from "@/lib/types";
import { CornerPad, Row } from "./shared";

/** The value the camera dropdown uses for "no camera chosen". */
const FIRST = "__first";

export function FacecamTab() {
  const { config, cameras, camStatus, patchConfig, refreshCameras } = useEngine(
    useShallow((s) => ({
      config: s.config,
      cameras: s.cameras,
      camStatus: s.camStatus,
      patchConfig: s.patchConfig,
      refreshCameras: s.refreshCameras,
    })),
  );
  const cam = config.facecam;
  const patch = (p: Partial<FacecamConfig>) => patchConfig({ facecam: { ...cam, ...p } });
  const [sizeDraft, setSizeDraft] = useState<number | null>(null);
  const [delayDraft, setDelayDraft] = useState<number | null>(null);

  // The list when the tab opens, never in the store's start.
  useEffect(() => {
    void refreshCameras();
  }, [refreshCameras]);

  const chosen = cam.deviceId ?? FIRST;
  const known = cameras.some((c) => c.id === cam.deviceId);
  const cameraOptions = [
    { value: FIRST, label: "First camera" },
    ...cameras.map((c) => ({ value: c.id, label: c.name })),
    ...(cam.deviceId && !known
      ? [{ value: cam.deviceId, label: `${cam.deviceName ?? "Camera"} (not connected)` }]
      : []),
  ];
  const off = !cam.enabled;
  const state = camStatus?.state;

  return (
    <section>
      <SectionTitle title="Camera" />
      <Card className="divide-y divide-line">
        <Row label="Camera">
          <Toggle checked={cam.enabled} onChange={(enabled) => patch({ enabled })} />
        </Row>
        {cam.enabled && (
          <div className="p-4">
            <Preview layout={cam.layout} />
            <div className="mt-3 flex items-center justify-between gap-4">
              <p className="text-xs text-ink-muted">{state ? camStateLine(state) : "Starting…"}</p>
              {state?.kind === "blocked" && (
                <Button size="sm" onClick={() => inTauri && void api.openCameraPrivacy()}>
                  Open camera privacy
                </Button>
              )}
            </div>
          </div>
        )}
        <Row label="Device">
          <Select
            label="Device"
            value={chosen}
            options={cameraOptions}
            disabled={off}
            onChange={(id) => {
              const device = cameras.find((c) => c.id === id);
              patch(
                id === FIRST
                  ? { deviceId: null, deviceName: null }
                  : { deviceId: id, deviceName: device?.name ?? cam.deviceName },
              );
            }}
          />
        </Row>
        <Row label="Resolution">
          <Segmented
            value={cam.resolution}
            disabled={off}
            options={[
              { key: "p720", label: "720p" },
              { key: "p1080", label: "1080p" },
            ]}
            onChange={(resolution) => patch({ resolution })}
          />
        </Row>
        <Row label="In clips">
          <Toggle checked={cam.inClips} disabled={off} onChange={(inClips) => patch({ inClips })} />
        </Row>
        <Row label="In the console">
          <Toggle
            checked={cam.consolePreview}
            disabled={off}
            onChange={(consolePreview) => patch({ consolePreview })}
          />
        </Row>
      </Card>

      <SectionTitle title="Position" />
      <Card className="divide-y divide-line">
        <Row label="Corner">
          <CornerPad
            mark="cam"
            value={cornerOf(cam.layout)}
            disabled={off}
            onChange={(corner) => patch({ layout: { ...cam.layout, ...layoutAt(corner) } })}
          />
        </Row>
        <Row label="Size">
          <div className={off ? "pointer-events-none flex w-56 items-center gap-3 opacity-40" : "flex w-56 items-center gap-3"}>
            <Slider
              label="Size"
              value={Math.round((sizeDraft ?? cam.layout.size) * 100)}
              min={12}
              max={50}
              onChange={(v) => setSizeDraft(v / 100)}
              onRelease={() => setSizeDraft(null)}
              onCommit={(v) => {
                setSizeDraft(null);
                patch({ layout: { ...cam.layout, size: v / 100 } });
              }}
            />
            <span className="w-12 shrink-0 text-right font-mono text-xs text-ink-muted">
              {Math.round((sizeDraft ?? cam.layout.size) * 100)} %
            </span>
          </div>
        </Row>
        <Row label="Shape">
          <Segmented<CamShape>
            value={cam.layout.shape}
            disabled={off}
            options={[
              { key: "rect", label: "Rectangle" },
              { key: "rounded", label: "Rounded" },
              { key: "circle", label: "Circle" },
            ]}
            onChange={(shape) => patch({ layout: { ...cam.layout, shape } })}
          />
        </Row>
        <Row label="Mirror">
          <Toggle
            checked={cam.layout.mirror}
            disabled={off}
            onChange={(mirror) => patch({ layout: { ...cam.layout, mirror } })}
          />
        </Row>
        <Row label="Delay">
          <div className={off ? "pointer-events-none flex w-56 items-center gap-3 opacity-40" : "flex w-56 items-center gap-3"}>
            <Slider
              label="Delay"
              value={delayDraft ?? cam.delayMs}
              min={-300}
              max={300}
              step={10}
              onChange={setDelayDraft}
              onRelease={() => setDelayDraft(null)}
              onCommit={(delayMs) => {
                setDelayDraft(null);
                patch({ delayMs });
              }}
            />
            <span className="w-16 shrink-0 text-right font-mono text-xs text-ink-muted">
              {(delayDraft ?? cam.delayMs).toString().replace("-", "−")} ms
            </span>
          </div>
        </Row>
      </Card>
    </section>
  );
}

/** The live picture, shaped and mirrored like the clip will show it. */
function Preview({ layout }: { layout: FacecamConfig["layout"] }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    if (!canvas.current || !inTauri) return;
    return startCamPreview(canvas.current, { onlyWhileVisible: true });
  }, []);
  const radius = layout.shape === "circle" ? "9999px" : layout.shape === "rounded" ? "12%" : "0";
  return (
    <div className="flex justify-center">
      <canvas
        ref={canvas}
        width={320}
        height={180}
        className="bg-elevated"
        style={{
          width: layout.shape === "circle" ? 180 : 320,
          height: 180,
          objectFit: "cover",
          borderRadius: radius,
          transform: layout.mirror ? "scaleX(-1)" : undefined,
        }}
      />
    </div>
  );
}
