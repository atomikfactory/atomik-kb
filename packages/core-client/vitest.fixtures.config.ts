import { defineConfig } from "vitest/config";

// `pnpm --filter @atomik-kb/core-client fixtures`: regenerates the fixtures this package
// writes (`fixtures/doc/page-v1.from-js.snapshot.bin`). Separate from `test`, which only
// reads fixtures.
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.fixture.ts"],
  },
});
