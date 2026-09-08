import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import path from "path";

const host = process.env.TAURI_DEV_HOST;
const tauriPlatform = process.env.TAURI_ENV_PLATFORM || "";

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  define: {
    __AGENTS_LAUNCHER_PLATFORM__: JSON.stringify(tauriPlatform),
  },
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "src"),
    },
  },
  server: {
    port: 5173,
    strictPort: false,
    host: host || "127.0.0.1",
    hmr: host ? { protocol: "ws", host, port: 5174 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari16",
    minify: !process.env.TAURI_ENV_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    // The CodeMirror runtime (editor chunk) is one lazy-loaded bundle that
    // cannot be split further per package; assets load from local disk in the
    // desktop app, so the web-oriented 500 kB heuristic doesn't apply.
    chunkSizeWarningLimit: 600,
    rollupOptions: {
      output: {
        manualChunks: {
          vue: ["vue", "pinia"],
          xterm: ["@xterm/xterm", "@xterm/addon-fit", "@xterm/addon-web-links"],
          tauri: ["@tauri-apps/api", "@tauri-apps/plugin-dialog", "@tauri-apps/plugin-shell"],
          // FilePanel's editor/markdown vendors, split out so the lazy
          // FilePanel chunk stays small and the vendor chunks are cacheable.
          editor: [
            "@codemirror/view",
            "@codemirror/state",
            "@codemirror/commands",
            "@codemirror/language",
            "@codemirror/lang-markdown",
            "@codemirror/language-data",
            "@lezer/highlight",
          ],
          markdown: ["markdown-it", "highlight.js"],
        },
      },
    },
  },
});
