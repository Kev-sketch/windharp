import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { defineConfig } from 'vite';
import solidPlugin from 'vite-plugin-solid';
import viteResolve from 'vite-plugin-resolve';

import { i18nImporter } from './vite-plugins/i18n-importer.mts';
import pluginLoader from './vite-plugins/plugin-loader.mts';
import {
  pluginVirtualModuleGenerator,
} from './vite-plugins/plugin-importer.mts';
import { manifestGenerator } from './vite-plugins/manifest-generator.mts';

const __dirname = dirname(fileURLToPath(import.meta.url));

const resolveAlias = {
  '@': resolve(__dirname, './src'),
  '@assets': resolve(__dirname, './assets'),
};

// node: builtins that plugin code may reference; they must stay external so the
// renderer IIFE never tries to bundle them.
const nodeBuiltins = [
  'assert', 'async_hooks', 'buffer', 'child_process', 'cluster', 'console',
  'constants', 'crypto', 'dgram', 'diagnostics_channel', 'dns', 'domain',
  'events', 'fs', 'http', 'http2', 'https', 'inspector', 'module', 'net',
  'os', 'path', 'path/posix', 'path/win32', 'perf_hooks', 'process', 'punycode',
  'querystring', 'readline', 'repl', 'stream', 'stream/promises',
  'stream/web', 'string_decoder', 'sys', 'timers', 'tls', 'trace_events',
  'tty', 'url', 'util', 'util/types', 'v8', 'vm', 'wasi', 'worker_threads',
  'zlib',
];

const nodePrefixed = nodeBuiltins.flatMap((m) => [`node:${m}`, m]);

// Renderer-only build for the Tauri webview.
// The Node/Electron main-process bundle is gone; all backend logic lives in src-tauri (Rust).
// Shape mirrors the legacy electron-vite renderer build: single IIFE bundle from
// src/renderer.ts, everything external (electron/node) resolved by the shim at runtime.
export default defineConfig(({ mode }) => {
  const isDev = mode === 'development';

  return {
    plugins: [
      pluginLoader('renderer'),
      manifestGenerator(),
      viteResolve({
        'virtual:i18n': i18nImporter(),
        'virtual:plugins': pluginVirtualModuleGenerator('renderer'),
      }),
      solidPlugin(),
    ],
    publicDir: 'assets',
    define: {
      // The plugin-loader virtual module consults import.meta.env.MODE at runtime;
      // mirror the old electron-vite behaviour so plugins branch correctly.
      __IS_TAURI__: JSON.stringify(true),
    },
    resolve: {
      alias: resolveAlias,
    },
    build: {
      outDir: 'dist',
      target: 'esnext',
      assetsInlineLimit: 100000000,
      chunkSizeWarningLimit: 20000,
      minify: !isDev,
      lib: {
        entry: resolve(__dirname, './src/renderer.ts'),
        formats: ['iife'],
        fileName: () => 'index.js',
        name: 'PearRenderer',
      },
      rollupOptions: {
        external: ['electron', ...nodePrefixed],
        output: {
          // IIFE single file; every asset inlined.
          inlineDynamicImports: true,
        },
      },
    },
    clearScreen: false,
    server: {
      port: 5173,
      strictPort: true,
      // YT page injects the bundle via <script>; dev server must answer its CORS.
      cors: {
        origin: 'https://music.youtube.com',
      },
    },
  };
});
