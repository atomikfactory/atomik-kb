// Test-only access to files elsewhere in the repository (Node `fs`; this directory is
// excluded from the build): the BIP-39 word list kb-crypto vendors and the crypto goldens
// under `fixtures/crypto/`, written by `cargo run -p kb-crypto --example make_vectors`.

import { existsSync, readFileSync } from "node:fs";

const REPO_ROOT = new URL("../../../../", import.meta.url);

/** Whether a file exists, by its path from the repository root. */
export function repoFileExists(path: string): boolean {
  return existsSync(new URL(path, REPO_ROOT));
}

/** A text file by its path from the repository root, line endings normalised to `\n`. */
export function readRepoText(path: string): string {
  return readFileSync(new URL(path, REPO_ROOT), "utf8").replace(/\r\n/gu, "\n");
}
