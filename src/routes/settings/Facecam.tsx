import { useEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { api, fileUrl, inTauri } from "@/lib/ipc";
import { Button } from "@/components/ui/Button";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Segmented, Select, Slider, Toggle } from "@/components/ui/Controls";
import { camStateLine, startCamPreview } from "@/lib/facecam";
import { camBox, layoutFromRect, snapLayout } from "@/lib/camGeometry";
import { usePictureBox } from "@/lib/pictureBox";
import { RectLayer } from "@/components/ui/RectLayer";
import type { CamLayout, CamShape, FacecamConfig } from "@/lib/types";
import { Row } from "./shared";

/** The value the camera dropdown uses for "no camera chosen". */
const FIRST = "__first";

export function FacecamTab() {
  const { config, cameras, camStatus, vcamStatus, patchConfig, refreshCameras, setVirtualCamera, removeVirtualCamera } =
    useEngine(
      useShallow((s) => ({
        config: s.config,
        cameras: s.cameras,
        camStatus: s.camStatus,
        vcamStatus: s.vcamStatus,
        patchConfig: s.patchConfig,
        refreshCameras: s.refreshCameras,
        setVirtualCamera: s.setVirtualCamera,
        removeVirtualCamera: s.removeVirtualCamera,
      })),
    );
  // While Windows asks for admin rights and the setup runs.
  const [vcamBusy, setVcamBusy] = useState(false);
  const vcamDo = async (action: () => Promise<void>) => {
    setVcamBusy(true);
    try {
      await action();
    } finally {
      setVcamBusy(false);
    }
  };
  const cam = config.facecam;
  const patch = (p: Partial<FacecamConfig>) => patchConfig({ facecam: { ...cam, ...p } });
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
            <Placement layout={cam.layout} onCommit={(layout) => patch({ layout })} />
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
        <Row label="In recordings">
          <Toggle checked={cam.inRecordings} disabled={off} onChange={(inRecordings) => patch({ inRecordings })} />
        </Row>
        <Row label="In screenshots">
          <Toggle checked={cam.inScreenshots} disabled={off} onChange={(inScreenshots) => patch({ inScreenshots })} />
        </Row>
        <Row label="In the console">
          <Toggle
            checked={cam.consolePreview}
            disabled={off}
            onChange={(consolePreview) => patch({ consolePreview })}
          />
        </Row>
        <Row label="Virtual camera">
          <div className="flex items-center gap-3">
            {vcamStatus?.kind === "live" && <span className="text-xs text-ink-muted">In use</span>}
            {vcamStatus?.kind === "needsUpdate" && (
              <Button size="sm" disabled={vcamBusy} onClick={() => void vcamDo(() => setVirtualCamera(true))}>
                Update
              </Button>
            )}
            {vcamStatus && vcamStatus.kind !== "notInstalled" && !cam.virtualCamera && (
              <Button size="sm" variant="ghost" disabled={vcamBusy} onClick={() => void vcamDo(removeVirtualCamera)}>
                Remove
              </Button>
            )}
            <Toggle
              checked={cam.virtualCamera}
              disabled={off || vcamBusy}
              onChange={(on) => void vcamDo(() => setVirtualCamera(on))}
            />
          </div>
        </Row>
      </Card>

      <SectionTitle title="Position" />
      <Card className="divide-y divide-line">
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

/** The 1080p frame the camera's place is reckoned in. */
const FRAME = { width: 1920, height: 1080 };

/**
 * Where new clips get the camera: the live picture at its place on the
 * newest clip's thumbnail, dragged and resized as in the clip editor, stored
 * when it is let go.
 */
function Placement({ layout, onCommit }: { layout: CamLayout; onCommit: (layout: CamLayout) => void }) {
  const stage = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [draft, setDraft] = useState<CamLayout | null>(null);
  const backdrop = useEngine((s) => s.clips.find((c) => c.thumbPath && !c.screenshot)?.thumbPath ?? null);
  const box = usePictureBox(stage, FRAME.width, FRAME.height);
  useEffect(() => {
    if (!canvas.current || !inTauri) return;
    return startCamPreview(canvas.current, { onlyWhileVisible: true });
  }, []);
  const shown = draft ?? layout;
  const place = camBox(shown, FRAME.width, FRAME.height);
  const radius = shown.shape === "circle" ? "50%" : shown.shape === "rounded" ? `${0.12 * place.height * (box?.scale ?? 0)}px` : 0;
  return (
    <div ref={stage} className="relative mx-auto aspect-video w-full max-w-md overflow-hidden rounded-inner bg-elevated">
      {backdrop && <img src={fileUrl(backdrop)} alt="" draggable={false} className="absolute inset-0 h-full w-full object-cover opacity-60" />}
      {/* Always there: the preview starts on mount, before the stage knows its size. */}
      <canvas
        ref={canvas}
        width={320}
        height={180}
        className="absolute bg-black object-cover"
        style={
          box
            ? {
                left: box.left + place.left * box.scale,
                top: box.top + place.top * box.scale,
                width: place.width * box.scale,
                height: place.height * box.scale,
                borderRadius: radius,
                transform: shown.mirror ? "scaleX(-1)" : undefined,
              }
            : { visibility: "hidden" }
        }
      />
      {/* Only once the stage has its size: mounted with it, the layer measured
          before the stage's ref was set and never again. */}
      {box && (
        <RectLayer
          mode="box"
          stage={stage}
          width={FRAME.width}
          height={FRAME.height}
          ratio={shown.shape === "circle" ? 1 : 16 / 9}
          rect={{ x: place.left, y: place.top, width: place.width, height: place.height }}
          onRect={(rect) => setDraft(snapLayout({ ...shown, ...layoutFromRect(rect, FRAME, shown.shape) }))}
          onRelease={() => {
            if (draft) onCommit(draft);
            setDraft(null);
          }}
        />
      )}
    </div>
  );
}
