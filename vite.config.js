import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { readFileSync } from 'fs';

// tauri.conf.json is the single source of truth for the app version.
// We inject it at build time so it's available in browser dev mode too.
const tauriConf = JSON.parse(readFileSync('./src-tauri/tauri.conf.json', 'utf8'));

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: true
  },
  envPrefix: ['VITE_', 'TAURI_'],
  define: {
    'import.meta.env.VITE_APP_VERSION': JSON.stringify(tauriConf.version),
  },
  build: {
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari13',
    minify: !process.env.TAURI_ENV_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG
  }
});
