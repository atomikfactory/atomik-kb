// Copies the built editor (dist/) into the Android app's assets: apps/android/app/src/main/assets/editor/
// (served by WebViewAssetLoader at https://appassets.androidplatform.net/editor/). Run `pnpm build` first.
import { cp, mkdir, rm, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const dist = join(root, "dist");
const target = join(root, "..", "android", "app", "src", "main", "assets", "editor");

try {
  await stat(join(dist, "index.html"));
} catch {
  console.error("dist/index.html is missing: run `pnpm --filter @atomik-kb/mobile-editor build` first");
  process.exit(1);
}
await rm(target, { recursive: true, force: true });
await mkdir(target, { recursive: true });
await cp(dist, target, { recursive: true });
console.log(`copied ${dist} -> ${target}`);
