import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [vue()],

  // Keep a project-specific cache so dependency graphs from older imports (for
  // example the former all-icons barrel) cannot be reused by the dev server.
  cacheDir: "node_modules/.vite-snapclip",

  // Limit dependency discovery to the application entry. Direct icon-module
  // imports keep the graph bounded without disabling pre-bundling entirely;
  // disabling it makes WebView2 request every dependency module separately.
  optimizeDeps: {
    entries: ["src/main.ts"],
    include: [
      "vue",
      "pinia",
      "@tanstack/vue-virtual",
      "@tauri-apps/api/core",
      "@tauri-apps/api/event",
      "@tauri-apps/api/window",
    ],
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. Keep the watcher restricted to the web app. This repository also
      // contains Rust build output, OCR models, old comparisons and fixtures;
      // letting chokidar walk those trees can consume a gigabyte before the
      // first WebView request is served.
      ignored: [
        "**/src-tauri/**",
        "**/node_modules/**",
        "**/.git/**",
        "**/.comparison-old/**",
        "**/comparison-*/**",
        "**/refer/**",
        "**/OCR-Model/**",
        "**/Formula-TestSet/**",
        "**/target/**",
        "**/dist/**",
        "**/.playwright-mcp/**",
        "**/.spec-workflow/**",
      ],
    },
  },
}));
