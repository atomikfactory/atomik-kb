import react from "@vitejs/plugin-react";
import { defineConfig, loadEnv } from "vite";
import type { Plugin } from "vite";
import wasm from "vite-plugin-wasm";

import { excalidrawAssets } from "./excalidraw-assets";

// The Tauri CLI exports TAURI_ENV_* (platform, debug flag, ...) to its hook commands;
// expose them to the frontend next to Vite's own VITE_* variables.
const ENV_PREFIXES = ["VITE_", "TAURI_ENV_"];

/** The font files of the math block's stylesheet, which KaTeX lists three times each (woff2, woff, ttf). */
const KATEX_STYLESHEET = /katex\.min\.css$/u;
const KATEX_FALLBACK_FONTS = /,url\(fonts\/[^)]+\.(?:woff|ttf)\) format\("(?:woff|truetype)"\)/gu;

/**
 * KaTeX's stylesheet (loaded with the first equation, `packages/ui/src/editor/math.ts`) lists
 * every font as woff2, woff and ttf, and the bundler would emit all three: about 1.2 MB of
 * which a webview loads a quarter. Every webview the app runs in (WebView2, WKWebView,
 * WebKitGTK, and the browsers of the web build) loads woff2, so only that format stays.
 */
function katexWoff2Only(): Plugin {
  return {
    name: "atomik-katex-woff2-only",
    enforce: "pre",
    transform(code, id) {
      return KATEX_STYLESHEET.test(id.split("?")[0] ?? id) ? code.replace(KATEX_FALLBACK_FONTS, "") : null;
    },
  };
}

// https://v2.tauri.app/start/frontend/vite/
export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, ".", ENV_PREFIXES);
  const debug = env.TAURI_ENV_DEBUG === "true";
  // Vitest runs this config too (mode "test"), in Node.js, where loro-crdt's own Node.js
  // build is the right one.
  const browser = mode !== "test";

  return {
    // The page editor runs loro-crdt (ADR-0011, ADR-0014): its WebAssembly module is an
    // ES module import that vite-plugin-wasm instantiates.
    plugins: [react(), wasm() as Plugin, katexWoff2Only(), excalidrawAssets()],

    resolve: {
      alias: browser
        ? [
            // loro-crdt's "browser" build loads its wasm with a synchronous XMLHttpRequest,
            // which the webview's CSP (`connect-src`) refuses; the "bundler" build imports it.
            { find: /^loro-crdt$/, replacement: "loro-crdt/bundler" },
          ]
        : [],
    },

    // Pre-bundling would have to understand the wasm import; serve it as it is.
    optimizeDeps: { exclude: ["loro-crdt"] },

    // Keep the Rust compiler's output visible in the terminal.
    clearScreen: false,
    envPrefix: ENV_PREFIXES,

    server: {
      // tauri.conf.json points `devUrl` here; a different port would show a blank window.
      port: 1420,
      strictPort: true,
      watch: {
        // Rust changes are watched by `tauri dev`, not by Vite.
        ignored: ["**/src-tauri/**"],
      },
    },

    build: {
      // WebView2 is Chromium; WKWebView and WebKitGTK are WebKit. Both targets support the
      // top-level await of the wasm glue.
      target: env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari15",
      // The wasm goes into the bundle as a data URL: the CSP's `connect-src` has no
      // 'self', so fetching it as a separate asset would be refused.
      // Fonts are never inlined either: `font-src 'self'` refuses data: URLs, and KaTeX's
      // smallest font (Size3, 3.6 kB) is under the default limit.
      assetsInlineLimit: (file: string) => (file.endsWith(".wasm") ? true : /\.(?:woff2?|ttf|otf)$/u.test(file) ? false : undefined),
      // The app is loaded from disk, not over a network: one large chunk is fine.
      chunkSizeWarningLimit: 8192,
      // Readable output while debugging a debug build.
      ...(debug ? { minify: false, sourcemap: true } : {}),
    },
  };
});
