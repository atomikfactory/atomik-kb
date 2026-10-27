// Builds `crates/kb-wasm` for the browser and writes the JavaScript glue the viewer imports (M13 slice 4, ADR-0051):
//
//   1. cargo build --release --target wasm32-unknown-unknown -p kb-wasm   (no clang: kb-crypto and kb-sync only)
//   2. wasm-bindgen --target web --out-dir apps/viewer/src/wasm --out-name kb_wasm  <the .wasm cargo made>
//
// The output (`src/wasm/kb_wasm.js`, `kb_wasm.d.ts`, `kb_wasm_bg.wasm`) is generated, never edited, and ignored by git
// and eslint: it is made again by every `pnpm build` of the viewer, from the Rust that is in the tree, so it cannot
// drift from `crates/kb-wasm`, `kb-sync` and `kb-crypto`. The other scripts of the package (`test`, `typecheck`,
// `lint`) pass `--if-missing`: they build it only when it is not there at all.
//
// Needs the `wasm32-unknown-unknown` target (`rust-toolchain.toml` lists it) and `wasm-bindgen-cli` in exactly the
// version of the `wasm-bindgen` crate in `Cargo.lock` (0.2.129): `cargo install wasm-bindgen-cli --version 0.2.129
// --locked`. On the development machine run it from a shell that has `scripts/dev-env.ps1` dot-sourced (cargo's
// path, and the download proxy when it is needed).

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const viewerRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = join(viewerRoot, "..", "..");
const outDir = join(viewerRoot, "src", "wasm");
const glue = join(outDir, "kb_wasm.js");
const wasmFile = join(repoRoot, "target", "wasm32-unknown-unknown", "release", "kb_wasm.wasm");

if (process.argv.includes("--if-missing") && existsSync(glue) && existsSync(join(outDir, "kb_wasm_bg.wasm"))) {
  process.exit(0);
}

function run(command, args, hint) {
  console.log(`> ${command} ${args.join(" ")}`);
  const result = spawnSync(command, args, { cwd: repoRoot, stdio: "inherit" });
  if (result.error !== undefined || result.status !== 0) {
    console.error(`\nbuild-wasm: \`${command}\` failed${result.error === undefined ? "" : `: ${result.error.message}`}. ${hint}`);
    process.exit(result.status ?? 1);
  }
}

run(
  "cargo",
  ["build", "--release", "--target", "wasm32-unknown-unknown", "-p", "kb-wasm"],
  "Is the Rust toolchain on PATH (on the development machine: `. .\\scripts\\dev-env.ps1`) with the wasm32-unknown-unknown target?",
);
run(
  "wasm-bindgen",
  ["--target", "web", "--out-dir", outDir, "--out-name", "kb_wasm", wasmFile],
  "wasm-bindgen-cli must be exactly 0.2.129: `cargo install wasm-bindgen-cli --version 0.2.129 --locked`.",
);
