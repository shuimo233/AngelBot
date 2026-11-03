import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import { resolve } from 'path';
import { availableParallelism } from 'node:os';

export default defineConfig({
  plugins: [react()],
  test: {
    // Avoid a Windows fork burst (spawn EBUSY / exhausted pool IDs) on
    // many-core desktops. Keep file isolation and every test enabled.
    minWorkers: 1,
    maxWorkers: Math.max(1, Math.min(4, availableParallelism() - 1)),
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
  },
  resolve: {
    alias: {
      '$types': resolve(__dirname, 'src/types.ts'),
      '$components': resolve(__dirname, 'src/components'),
      '$stores': resolve(__dirname, 'src/stores'),
      '$lib': resolve(__dirname, 'src/lib'),
    },
  },
});
