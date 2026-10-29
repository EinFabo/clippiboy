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
      <SectionTitle title="Facecam" />
      <Card className="divide-y divide-line">
        <Row label="Facecam" hint="Your webcam in your clips. Off, the camera is not opened at all.">
          <Toggle checked={cam.enabled} onChange={(enabled) => patch({ enabled })} />
        </Row>
        {cam.enabled && (
          <div className="p-4">
            <Preview layout={cam.layout} />
            <p className="mt-3 text-xs text-ink-muted">{state ? camStateLine(state) : "Starting…"}</p>
            {state?.kind === "blocked" && (
              <div className="mt-2 flex items-center justify-between gap-4">
                <p className="text-xs text-ink-faint">Windows blocks the camera for desktop apps</p>
                <Button size="sm" onClick={() => inTauri && void api.openCameraPrivacy()}>
                  Open camera privacy
                </Button>
              </div>
            )}
            {state?.kind === "taken" && (
              <p className="mt-2 text-xs text-ink-faint">
                Windows 11: Settings › Bluetooth &amp; devices › Cameras › your camera › Allow multiple apps
                to use camera at the same time
              </p>
            )}
            <p className="mt-2 text-xs text-ink-faint">
              While the facecam is on, Discord and other apps can't use this camera.
            </p>
          </div>
        )}
      </Card>

      <SectionTitle title="Camera" />
      <Card className="divide-y divide-line">
        <Row label="Camera" hint="OBS Virtual Camera and other cameras only older programs can see are not supported yet.">
          <Select
            label="Camera"
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
        <Row label="Resolution" hint="Applies the next time the camera starts.">
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
      </Card>

      <SectionTitle title="Where it appears" />
      <Card className="divide-y divide-line">
        <Row label="Clips" hint="Saved clips get the cam a few seconds after saving.">
          <Toggle checked={cam.inClips} disabled={off} onChange={(inClips) => patch({ inClips })} />
        </Row>
        <Row label="Console preview" hint="Shows where the cam will sit while the console is open.">
          <Toggle
            checked={cam.consolePreview}
            disabled={off}
            onChange={(consolePreview) => patch({ consolePreview })}
          />
        </Row>
      </Card>

      <SectionTitle title="Default position" />
      <Card className="divide-y divide-line">
        <Row label="Corner">
          <CornerPad
            mark="cam"
            value={cornerOf(cam.layout)}
            disabled={off}
            onChange={(corner) => patch({ layout: { ...cam.layout, ...layoutAt(corner) } })}
          />
        </Row>
        <Row label="Size" hint={`${Math.round((sizeDraft ?? cam.layout.size) * 100)} % of the picture's height`}>
          <div className={off ? "pointer-events-none w-48 opacity-40" : "w-48"}>
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
        <Row label="Mirror" hint="Off, others see what the camera sees.">
          <Toggle
            checked={cam.layout.mirror}
            disabled={off}
            onChange={(mirror) => patch({ layout: { ...cam.layout, mirror } })}
          />
        </Row>
      </Card>

      <SectionTitle title="Sync" />
      <Card className="divide-y divide-line">
        <Row
          label="Cam delay"
          hint="Clap once in view and save a clip. If you hear the clap before you see it, move this to the left."
        >
          <div className={off ? "pointer-events-none flex w-56 items-center gap-3 opacity-40" : "flex w-56 items-center gap-3"}>
            <Slider
              label="Cam delay"
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
