import { useEffect, useState } from "react";
import { useEngine } from "@/store";
import { Card, SectionTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { api, inTauri } from "@/lib/ipc";
import { UpdateProgressBar, updateProgressText } from "@/components/UpdateProgress";
import { Row } from "./shared";

export function AboutTab() {
  return (
    <section>
      <SectionTitle title="Version" />
      <Updates />
    </section>
  );
}

/** Version, update check and installation. */
function Updates() {
  const [version, setVersion] = useState("0.1.0");
  // Shared with the notice at the top of every page, so a check here and the
  // hourly one in the core land in the same place.
  const update = useEngine((s) => s.update);
  const setUpdate = useEngine((s) => s.setUpdate);
  // `null` means: not checked yet. Otherwise the text under the row.
  const [state, setState] = useState<"idle" | "checking" | "current" | "failed">(
    "idle",
  );
  const progress = useEngine((s) => s.updateProgress);
  const installUpdate = useEngine((s) => s.installUpdate);
  const installing = progress !== null;
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!inTauri) return;
    api.appVersion().then(setVersion).catch(() => {});
  }, []);

  const check = async () => {
    setState("checking");
    setError(null);
    try {
      const found = await api.checkUpdate();
      setUpdate(found);
      setState(found ? "idle" : "current");
    } catch (err) {
      setState("failed");
      setError(String(err));
    }
  };

  const install = async () => {
    setError(null);
    try {
      // Does not come back: Windows quits the app and starts the installer.
      await installUpdate();
    } catch (err) {
      setError(String(err));
    }
  };

  return (
    <>
      <Card className="divide-y divide-line">
        <Row
          label={`ClippiBoy ${version}`}
          hint={
            progress
              ? updateProgressText(progress)
              : update
                ? `Version ${update.version} is available`
                : state === "current"
                  ? "Up to date"
                  : state === "failed"
                    ? "The update check failed"
                    : "Updates come from GitHub Releases and are signed"
          }
        >
          <div className="flex gap-2">
            <Button
              size="sm"
              variant="secondary"
              disabled={!inTauri || state === "checking" || installing}
              onClick={check}
            >
              {state === "checking" ? "Checking …" : "Check for updates"}
            </Button>
            {update && (
              <Button size="sm" variant="primary" disabled={installing} onClick={install}>
                {installing ? "Installing …" : `Update to ${update.version}`}
              </Button>
            )}
          </div>
        </Row>
        {progress && (
          <div className="px-5 pb-4">
            <UpdateProgressBar progress={progress} />
          </div>
        )}
        {update?.notes && (
          <div className="p-5">
            <p className="text-xs whitespace-pre-line text-ink-muted">
              {update.notes}
            </p>
          </div>
        )}
      </Card>
      {update && (
        <p className="mt-3 text-xs text-ink-faint">
          Installing quits ClippiBoy and runs the installer — a running buffer is
          stopped cleanly first.
        </p>
      )}
      {error && <p className="mt-3 text-xs text-live">{error}</p>}
    </>
  );
}
