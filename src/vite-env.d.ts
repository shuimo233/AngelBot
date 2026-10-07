/// <reference types="vite/client" />

declare global {
  interface Window {
    __TAURI__: {
      invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
    };
  }
}

export {};
