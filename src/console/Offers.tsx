import { useState } from "react";
import { cn } from "@/lib/cn";
import { megabytes, shareApi, useShare } from "@/lib/share";
import type { Transfer } from "@/lib/types";

/**
 * Clips, die ein Freund schicken will — oben in der Konsole, zum Annehmen im
 * Spiel. Das Banner darüber kann niemand anklicken (es lässt jeden Klick ans
 * Spiel durch); es sagt nur, dass die Antwort hier liegt.
 *
 * Kostet kein eigenes Backend: `share-state` geht an jedes Fenster, und der
 * Store in `lib/share.ts` hört in diesem hier genauso zu wie im Hauptfenster.
 */
export function Offers() {
  const transfers = useShare((s) => s.transfers);
  const shown = transfers.filter((t) => t.direction === "in" && (t.stage === "asking" || t.stage === "moving"));
  if (shown.length === 0) return null;
  return (
    <div className="pointer-events-auto absolute top-8 left-1/2 flex w-[380px] -translate-x-1/2 flex-col gap-2">
      {shown.map((t) => (
        <Offer key={t.id} transfer={t} />
      ))}
    </div>
  );
}

function Offer({ transfer: t }: { transfer: Transfer }) {
  const [busy, setBusy] = useState(false);
  const run = (action: Promise<void>) => {
    setBusy(true);
    action.finally(() => setBusy(false));
  };
  const share = t.size > 0 ? t.moved / t.size : 0;
  const what = t.screenshot ? "einen Screenshot" : t.recording ? "eine Aufnahme" : "einen Clip";

  return (
    <div className="cb-glass rounded-[18px] px-4 py-3.5">
      <p className="truncate text-[13px] font-semibold">
        {t.stage === "asking" ? `${t.friendName} will dir ${what} schicken` : `Empfange von ${t.friendName} …`}
      </p>
      <p className="truncate text-xs text-ink-muted">
        {t.name} · {megabytes(t.size)}
        {t.game && ` · ${t.game}`}
      </p>
      {t.stage === "asking" ? (
        <div className="mt-3 flex justify-end gap-2">
          <button
            disabled={busy}
            onClick={() => run(shareApi.decline(t.id))}
            className="rounded-pill px-3.5 py-1.5 text-xs font-semibold text-ink-muted transition-colors hover:bg-white/10 hover:text-ink"
          >
            Ablehnen
          </button>
          <button
            disabled={busy}
            onClick={() => run(shareApi.accept(t.id))}
            className={cn(
              "rounded-pill bg-white px-3.5 py-1.5 text-xs font-semibold text-black transition-opacity",
              busy && "opacity-60",
            )}
          >
            Annehmen
          </button>
        </div>
      ) : (
        <div className="mt-2.5 flex items-center gap-3">
          <div className="h-1 flex-1 overflow-hidden rounded-pill bg-white/10">
            <div
              className="h-full rounded-pill bg-accent transition-[width] duration-200"
              style={{ width: `${Math.min(Math.max(share, 0), 1) * 100}%` }}
            />
          </div>
          <span className="text-xs text-ink-muted tabular-nums">{Math.floor(share * 100)} %</span>
        </div>
      )}
    </div>
  );
}
