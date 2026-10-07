import { create } from 'zustand';

export type AppPage = 'chat' | 'memory' | 'tasks' | 'knowledge' | 'persona';

interface NavigationState {
  currentPage: AppPage;
  setPage: (page: AppPage) => void;
}

export const useNavigationStore = create<NavigationState>((set) => ({
  currentPage: 'chat',
  setPage: (page) => set({ currentPage: page }),
}));
