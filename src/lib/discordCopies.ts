import { create } from "zustand";

/** Clips whose "Copy for Discord" is shrinking right now. */
export const useDiscordCopies = create<{ busy: ReadonlySet<string> }>(() => ({ busy: new Set() }));

/** Marks a clip as shrinking (or done); the set is replaced so subscribers see it. */
export function setDiscordBusy(id: string, on: boolean): void {
  const next = new Set(useDiscordCopies.getState().busy);
  if (on) next.add(id);
  else next.delete(id);
  useDiscordCopies.setState({ busy: next });
}

export function isDiscordBusy(id: string): boolean {
  return useDiscordCopies.getState().busy.has(id);
}
