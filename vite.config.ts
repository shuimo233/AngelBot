import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { resolve } from 'path';

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '$types': resolve(__dirname, 'src/types.ts'),
      '$components': resolve(__dirname, 'src/components'),
      '$stores': resolve(__dirname, 'src/stores'),
      '$lib': resolve(__dirname, 'src/lib'),
    },
  },
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    // Keep the Vite listener aligned with Tauri's dev URL on Windows.  Letting
    // Vite choose the default can bind only to IPv6 (::1), while `localhost`
    // resolves to IPv4 on some machines and leaves the desktop window blank.
    host: host || '127.0.0.1',
    hmr: host
      ? {
          protocol: 'ws',
          host,
          port: 5174,
        }
      : undefined,
    watch: {
      ignored: ['**/src-tauri/**'],
    },
    proxy: {
      // 转发 Dev HTTP IPC 到后端（绕过 webview localhost 限制）
      '/__tauri__': {
        target: 'http://127.0.0.1:1421',
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/__tauri__/, ''),
      },
    },
  },
  build: {
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari14',
    minify: !process.env.TAURI_ENV_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
