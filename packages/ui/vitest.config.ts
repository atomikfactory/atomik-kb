import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
    setupFiles: ["./src/test/setup.ts"],
    // Above asyncUtilTimeout (20 s, test/setup.ts) so a wait that gives up says what it waited for, not "timed out".
    testTimeout: 60_000,
    // One jsdom worker per core (31 on the 32-thread development machine) made worker start-up
    // time out under load ("Timeout waiting for worker to respond"); half the cores is as fast.
    maxWorkers: "50%",
  },
});
