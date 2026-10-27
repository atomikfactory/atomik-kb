import { defineConfig } from "vitest/config";

// `pnpm --filter @atomik-kb/ui fixtures`: regenerates the fixtures the UI owns
// (`fixtures/doc/block-schema-v1.json`) from the live code. Not part of `test`.
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.fixtures.ts"],
  },
});
