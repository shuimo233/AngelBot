import { create } from 'zustand';

export type AppPage = 'chat' | 'memory' | 'tasks' | 'knowledge' | 'persona';
export interface AutomationTarget { automationId: string; workspaceId: string; }

interface NavigationState {
  currentPage: AppPage;
  automationTarget: AutomationTarget | null;
  setPage: (page: AppPage) => void;
  openAutomation: (target: AutomationTarget) => void;
}

export const useNavigationStore = create<NavigationState>((set) => ({
  currentPage: 'chat',
  automationTarget: null,
  setPage: (page) => set({ currentPage: page, automationTarget: null }),
  openAutomation: (target) => set({ currentPage: 'tasks', automationTarget: { ...target } }),
}));
