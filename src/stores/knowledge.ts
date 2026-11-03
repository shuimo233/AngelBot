import { create } from 'zustand';

export interface KnowledgeEntry {
  id: string;
  content: string;
  category: string;
  importance: number;
  source: string;
  createdAt: number;
  updatedAt: number;
  scope: string;
  isPermanent: boolean;
  forgetStage: string;
}

interface KnowledgeState {
  entries: KnowledgeEntry[];
  isLoading: boolean;
  error: string | null;
  query: string;
  setQuery: (q: string) => void;
  loadKnowledge: () => Promise<void>;
}

let loadVersion = 0;
export const useKnowledgeStore = create<KnowledgeState>((set) => ({
  entries: [],
  isLoading: false,
  error: null,
  query: '',

  setQuery: (q) => set({ query: q }),

  loadKnowledge: async () => {
    const version = ++loadVersion;
    set({ isLoading: true, error: null });
    try {
      const { getMemories } = await import('$lib/commands/memory');
      const all = await getMemories();
      const knowledge = all
        .filter((m) => m.category === 'knowledge' || m.source === 'knowledge')
        .map((m) => ({
          id: m.id,
          content: m.content,
          category: m.category,
          importance: m.importance,
          source: m.source,
          createdAt: m.created_at,
          updatedAt: m.updated_at,
          scope: m.scope,
          isPermanent: m.is_permanent,
          forgetStage: m.forget_stage,
        }));
      if (version !== loadVersion) return;
      set({ entries: knowledge, isLoading: false });
    } catch (err) {
      if (version !== loadVersion) return;
      set({ error: err instanceof Error ? err.message : 'Failed to load', isLoading: false });
    }
  },
}));
