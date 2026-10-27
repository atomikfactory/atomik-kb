import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
    setupFiles: ["./src/test/setup.ts"],
    testTimeout: 15_000,
    // One jsdom worker per core (31 on the 32-thread development machine) made worker start-up
    // time out under load ("Timeout waiting for worker to respond"); half the cores is as fast.
    maxWorkers: "50%",
  },
});
