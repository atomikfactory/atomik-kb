import { defineConfig } from "vite";
import type { Plugin } from "vite";
import wasm from "vite-plugin-wasm";

// The viewer's build (M13 slice 4, ADR-0051): served by kb-server at `/view` under a strict policy (`VIEWER_CSP`: its own
// scripts and WebAssembly, requests to its own origin, images from its own origin and from blobs, nothing else).
export default defineConfig(({ mode }) => {
  // Vitest runs this config too (mode "test"), in Node.js, where loro-crdt's own Node.js build is the right one.
  const browser = mode !== "test";

  return {
    // kb-server serves the bundle at `/view` and its files below it (`/view/assets/...`): every address in the built
    // page is relative to that, and the page itself needs no `/v2` prefix knowledge (its requests are same-origin).
    base: "/view/",

    // The editor runs loro-crdt (ADR-0011): its WebAssembly module is an ES module import that vite-plugin-wasm
    // instantiates. No React plugin: its refresh preamble is an inline script, which the policy refuses.
    plugins: [wasm() as Plugin],

    resolve: {
      alias: browser
        ? [
            // loro-crdt's "browser" build loads its wasm with a synchronous XMLHttpRequest, which `connect-src`
            // is no place for; the "bundler" build imports it as a module.
            { find: /^loro-crdt$/, replacement: "loro-crdt/bundler" },
          ]
        : [],
    },

    // Pre-bundling would have to understand the wasm import; serve it as it is.
    optimizeDeps: { exclude: ["loro-crdt"] },

    // `pnpm dev` serves the page at /view/ and asks a local kb-server (`scripts/smoke/server.ps1`) for the link: same
    // origin as far as the page can tell, as it is in production.
    server: { port: 1430, strictPort: true, proxy: { "/v2": "http://127.0.0.1:8787" } },

    build: {
      // Top-level await in the wasm glue; every browser that runs WebAssembly under `wasm-unsafe-eval` has it.
      target: "es2022",
      // Nothing is inlined as a `data:` URL: the policy allows no `data:` source (`img-src 'self' blob:`), and the
      // wasm modules are fetched from the viewer's own origin.
      assetsInlineLimit: 0,
      // One page, one load: a large chunk is fine.
      chunkSizeWarningLimit: 8192,
    },
  };
});
