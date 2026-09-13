import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import path from "path";

const host = process.env.TAURI_DEV_HOST;
const tauriPlatform = process.env.TAURI_ENV_PLATFORM || "";

/**
 * Chrome 105 on Windows, Safari 16 elsewhere — the oldest engines this desktop
 * app actually runs in (WebView2 / WKWebView).
 */
const runtimeTarget = tauriPlatform === "windows" ? "chrome105" : "safari16";

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
  optimizeDeps: {
    esbuildOptions: {
      // The dependency pre-bundler has its own target and does NOT inherit
      // `build.target`: Vite hard-codes `ESBUILD_MODULES_TARGET`
      // (`es2020, edge88, firefox78, chrome87, safari14`) for it.
      //
      // That default breaks this project two ways at once:
      //   * `safari14` is an engine esbuild 0.28 refuses to lower destructuring
      //     for (Safari 14 has known destructuring bugs, so esbuild errors
      //     instead of emitting broken output). The pinned `overrides.esbuild`
      //     in package.json is newer than the `^0.25.0` vite declares, and the
      //     failure surfaces as `Transforming destructuring to the configured
      //     target environment ("chrome87", … "safari14") is not supported yet`
      //     on every xterm/CodeMirror dependency during `npm run dev`.
      //   * pre-bundled dependencies would be held to Chrome 87 while the
      //     production bundle targets Chrome 105 / Safari 16, so dev and build
      //     would disagree about what syntax is allowed.
      //
      // Aligning the optimizer with the app's real runtime fixes the errors and
      // removes the mismatch. Verified: esbuild rejects `safari14` but accepts
      // `safari14.1` and newer.
      target: runtimeTarget,
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    target: runtimeTarget,
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
