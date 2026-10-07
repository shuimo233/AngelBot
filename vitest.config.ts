import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import { resolve } from 'path';

export default defineConfig({
  plugins: [react()],
  test: {
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
