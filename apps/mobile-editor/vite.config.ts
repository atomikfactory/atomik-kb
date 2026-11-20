import { defineConfig } from "vite";
import type { Plugin } from "vite";
import wasm from "vite-plugin-wasm";

// The editor the Android app shows in a WebView. It is served from the APK's assets by WebViewAssetLoader at
// `https://appassets.androidplatform.net/editor/`, under the same strict policy as the web viewer: its own scripts and
// WebAssembly, images from itself and from blobs (attachments are decrypted into blobs), nothing else.
const POLICY =
  "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; img-src 'self' blob:; media-src 'self' blob:";

/** The policy as a <meta> tag; `vite dev` also injects styles as <style> elements and talks to its server by WebSocket. */
function policy(): Plugin {
  return {
    name: "mobile-editor-csp",
    transformIndexHtml: {
      order: "pre",
      handler(html, context) {
        const dev = context.server !== undefined;
        const content = dev ? `${POLICY.replace("connect-src 'self'", "connect-src 'self' ws:")}; style-src 'self' 'unsafe-inline'` : POLICY;
        return html.replace("__POLICY__", content);
      },
    },
  };
}

export default defineConfig(({ mode }) => {
  // Vitest runs this config too (mode "test"), in Node.js, where loro-crdt's own Node.js build is the right one.
  const browser = mode !== "test";

  return {
    // Relative addresses: the page is served below `/editor/` and nothing in it knows the prefix.
    base: "./",
    plugins: [wasm() as Plugin, policy()],
    resolve: {
      // loro-crdt's "browser" build loads its wasm with a synchronous XMLHttpRequest; the "bundler" build imports it.
      alias: browser ? [{ find: /^loro-crdt$/, replacement: "loro-crdt/bundler" }] : [],
    },
    optimizeDeps: { exclude: ["loro-crdt"] },
    server: { port: 1431, strictPort: true },
    build: {
      target: "es2022",
      // No `data:` sources under the policy.
      assetsInlineLimit: 0,
      chunkSizeWarningLimit: 8192,
    },
  };
});
