// Loads `crates/kb-wasm` (M13 slice 4): the JavaScript glue and the module `wasm-bindgen` made of it
// (`scripts/build-wasm.mjs`, into `src/wasm`, which git and eslint ignore). The module is a file of the viewer's own
// bundle, fetched from its own origin (`connect-src 'self'`) and compiled under `'wasm-unsafe-eval'`: nothing else of
// the policy is needed.

import init, * as glue from "./wasm/kb_wasm.js";
import wasmUrl from "./wasm/kb_wasm_bg.wasm?url";

import type { KbWasm } from "./kb-wasm";

let loading: Promise<KbWasm> | undefined;

/** The module, loaded once. */
export function loadKbWasm(): Promise<KbWasm> {
  loading ??= init({ module_or_path: wasmUrl }).then(() => glue);
  return loading;
}
