import { create } from "zustand";
import { neighbour, type NoticeId, type Wants } from "./notices";

interface Board {
  wants: Wants;
  picked: NoticeId | null;
  set: (id: NoticeId, wants: boolean) => void;
  step: (dir: 1 | -1) => void;
}

/** Which notice cards want to show, so only one of them does. */
export const useNoticeBoard = create<Board>((set) => ({
  wants: {},
  picked: null,
  set: (id, wants) =>
    set((s) => (!!s.wants[id] === wants ? s : { wants: { ...s.wants, [id]: wants } })),
  step: (dir) =>
    set((s) => {
      const next = neighbour(s.wants, s.picked, dir);
      return next ? { picked: next } : s;
    }),
}));
