import { useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { useEngine } from "@/store";
import { Card } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Toggle } from "@/components/ui/Controls";
import { api, inTauri } from "@/lib/ipc";
import { cn } from "@/lib/cn";
import { Row } from "./shared";

/**
 * The local port the Stream Deck plugin presses the buttons through.
 *
 * Nothing here normally needs touching: the app hands port and token to the
 * plugin through a file of its own, and both run on this machine under this
 * user. It is in the settings for the two cases where that is not enough — the
 * port is already taken, or the token got out and should stop working.
 */
export function StreamDeck() {
  const { config, patchConfig, regenerateControlToken } = useEngine(
    useShallow((s) => ({
      config: s.config,
      patchConfig: s.patchConfig,
      regenerateControlToken: s.regenerateControlToken,
    })),
  );
  const control = config.control;
  // Only while it is being typed: an empty field or a half-typed "4" must not
  // travel to the core as a port.
  const [port, setPort] = useState(String(control.port));
  const [shown, setShown] = useState(false);
  const [copied, setCopied] = useState(false);

  useEffect(() => setPort(String(control.port)), [control.port]);

  const commitPort = () => {
    const wanted = Number(port);
    if (!Number.isInteger(wanted) || wanted < 1024 || wanted > 65535) {
      setPort(String(control.port));
      return;
    }
    if (wanted !== control.port) {
      patchConfig({ control: { ...control, port: wanted } });
    }
  };

  const copy = async () => {
    if (!inTauri) return;
    await api.clipboardWriteText(control.token);
    setCopied(true);
    setTimeout(() => setCopied(false), 1600);
  };

  return (
    <>
      <Card className="divide-y divide-line">
        <Row
          label="Allow the Stream Deck"
          hint="Save a clip, take a screenshot and switch the buffer from a key — with the buffer level on it"
        >
          <Toggle
            checked={control.enabled}
            onChange={(enabled) => patchConfig({ control: { ...control, enabled } })}
          />
        </Row>
        <Row
          label="Port"
          hint="Only worth changing when something else already holds this one"
        >
          <input
            aria-label="Port"
            value={port}
            disabled={!control.enabled}
            inputMode="numeric"
            onChange={(e) => setPort(e.target.value)}
            onBlur={commitPort}
            onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
            className={cn(
              "w-24 rounded-inner border border-line bg-transparent px-2 py-1.5 text-right font-mono text-sm outline-none",
              "transition-colors hover:border-line-strong focus:border-line-strong focus:bg-elevated",
              "disabled:opacity-40",
            )}
          />
        </Row>
        <Row
          label="Token"
          hint="The plugin picks this up by itself — copy it only if you are entering it by hand"
        >
          <div className="flex items-center gap-2">
            <code className="max-w-[15rem] truncate font-mono text-xs text-ink-muted">
              {shown ? control.token : "•".repeat(18)}
            </code>
            <Button size="sm" variant="ghost" onClick={() => setShown(!shown)}>
              {shown ? "Hide" : "Show"}
            </Button>
            <Button size="sm" onClick={copy} disabled={!control.token}>
              {copied ? "Copied" : "Copy"}
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={regenerateControlToken}
            >
              New
            </Button>
          </div>
        </Row>
      </Card>
      <p className="mt-3 text-xs text-ink-faint">
        The port listens on this machine only and answers nothing without the
        token — it opens nothing towards the network. The plugin is at{" "}
        <span className="font-mono">github.com/EinFabo/clippiboy</span> under
        Releases; a new token takes effect at once, and the plugin on this
        machine picks it up by itself.
      </p>
    </>
  );
}
