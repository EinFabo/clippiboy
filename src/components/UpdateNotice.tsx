import { useState } from "react";
import { useEngine } from "@/store";
import { api } from "@/lib/ipc";
import { Card } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";

/**
 * A newer version is out — said on every page, not only in the settings.
 *
 * The check used to report only to the settings page, which is never open when
 * ClippiBoy comes up hidden in the tray at boot. So a release went unnoticed
 * until somebody went looking. Nothing installs by itself: that ends the app
 * and the buffer with it, so the click stays with the user.
 */
export function UpdateNotice() {
  const update = useEngine((s) => s.update);
  // "Later" holds for this version and this session. The hourly check keeps
  // reporting the same version, and it should not come straight back.
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!update || dismissed === update.version) return null;

  const install = async () => {
    setInstalling(true);
    setError(null);
    try {
      // Does not come back: Windows quits the app and starts the installer.
      await api.installUpdate();
    } catch (err) {
      setInstalling(false);
      setError(String(err));
    }
  };

  return (
    <Card className="mb-6 flex items-center gap-4 border-accent/40 bg-accent/10 p-4">
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium text-ink">
          ClippiBoy {update.version} is available
        </p>
        <p className="mt-0.5 text-xs text-ink-muted">
          {error ??
            `You have ${update.currentVersion}. Updating quits ClippiBoy — a running buffer is stopped first.`}
        </p>
      </div>
      <Button
        size="sm"
        variant="ghost"
        disabled={installing}
        onClick={() => setDismissed(update.version)}
      >
        Later
      </Button>
      <Button size="sm" variant="primary" disabled={installing} onClick={install}>
        {installing ? "Installing …" : "Update now"}
      </Button>
    </Card>
  );
}
