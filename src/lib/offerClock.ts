import { useEffect, useState } from "react";
import { create } from "zustand";
import { useShare } from "./share";
import { answerLeft, noteOffers, offerDeadline } from "./transferRows";

/**
 * The countdown on clip offers: to when the core says each runs out, or — for
 * one that does not say — from when it was first seen (`noteOffers`), noted
 * from the moment this module loads, with the share list, so an offer that
 * came in while the page was elsewhere counts from when it came.
 */
const useOfferSeen = create<{ seen: Record<string, number> }>(() => ({ seen: {} }));

const note = () => {
  const before = useOfferSeen.getState().seen;
  const seen = noteOffers(useShare.getState().transfers, before, Date.now());
  if (seen !== before) useOfferSeen.setState({ seen });
};
note();
useShare.subscribe(note);

/** Whole seconds left to answer this offer, ticking once a second. */
export function useAnswerLeft(id: string): number {
  const seenAt = useOfferSeen((s) => s.seen[id]);
  const expiresAt = useShare((s) => s.transfers.find((t) => t.id === id)?.expiresAt);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);
  return answerLeft(offerDeadline(expiresAt, seenAt ?? now), now);
}
