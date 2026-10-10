import { defineConfig } from 'vite';
import solid from 'vite-plugin-solid';
import { fileURLToPath } from 'node:url';

const r = (p: string) => fileURLToPath(new URL(p, import.meta.url));

export default defineConfig(({ isSsrBuild }) => ({
  plugins: [solid({ ssr: !!isSsrBuild })],
  resolve: {
    alias: {
      '@genoryn/shared': r('../../packages/shared/src/index.ts'),
      '~': r('./src'),
    },
  },
  build: {
    outDir: isSsrBuild ? 'dist/server' : 'dist/client',
    target: 'es2020',
    cssCodeSplit: true,
    sourcemap: false,
    assetsInlineLimit: 0,
    rollupOptions: isSsrBuild
      ? { output: { format: 'esm' } }
      : { output: { manualChunks: { solid: ['solid-js', 'solid-js/web', '@solidjs/router'] } } },
  },
  ssr: { noExternal: true },
  server: {
    port: 5173,
    proxy: { '/api': 'http://127.0.0.1:3000' },
  },
}));
