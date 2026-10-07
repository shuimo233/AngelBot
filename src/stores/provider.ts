/**
 * Provider Store - UI-level provider state
 *
 * Uses settings.ts as the source of truth for API config persistence.
 * ENV variables are read-only and taken from the backend on load.
 */

import { create } from 'zustand';
import type { ApiConfig, ProviderStatus } from '$types';

interface ProviderState {
  // ENV-detected provider info (read-only)
  envConfig: { provider: string; model: string; base_url: string; has_api_key: boolean } | null;

  // 当前活跃的配置（由 settings.ts 供给）
  activeConfig: ApiConfig | null;

  // 连接状态
  status: ProviderStatus;

  // 错误信息
  error: string | null;

  // 加载状态
  isLoading: boolean;

  // Actions
  loadEnvConfig: () => Promise<void>;
  setActiveConfig: (config: ApiConfig | null) => void;
  setStatus: (status: ProviderStatus) => void;
  setError: (error: string | null) => void;
}

export const useProviderStore = create<ProviderState>((set) => ({
  envConfig: null,
  activeConfig: null,
  status: 'unknown',
  error: null,
  isLoading: false,

  loadEnvConfig: async () => {
    set({ isLoading: true, error: null });
    try {
      const { getEnvConfig } = await import('$lib/commands');
      const envConfig = await getEnvConfig();
      set({ envConfig, isLoading: false });
    } catch {
      set({ isLoading: false });
    }
  },

  setActiveConfig: (config) => set({ activeConfig: config }),

  setStatus: (status) => set({ status }),

  setError: (error) => set({ error }),
}));
