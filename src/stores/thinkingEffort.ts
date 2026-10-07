import { create } from 'zustand';
import { persist } from 'zustand/middleware';

export type ThinkingEffort = 'low' | 'medium' | 'high';

interface ThinkingEffortState {
  effort: ThinkingEffort;
  setEffort: (effort: ThinkingEffort) => void;
}

export const useThinkingEffortStore = create<ThinkingEffortState>()(
  persist(
    (set) => ({
      effort: 'medium',
      setEffort: (effort) => set({ effort }),
    }),
    {
      name: 'angelbot-thinking-effort',
    }
  )
);
