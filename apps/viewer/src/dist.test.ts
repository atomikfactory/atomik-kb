// The built viewer fits the policy it is served under (M13 slice 4): no inline script or style, no remote address, no
// `data:` address, every asset hashed. It reads `dist/`, which `pnpm build` makes (and checks with the same function at its
// end); the test says so when there is no build to look at, and passes only when the build is there and fits.

import { existsSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { checkDist } from "../scripts/check-dist.mjs";

const dist = join(fileURLToPath(new URL(".", import.meta.url)), "..", "dist");

describe("the build under the viewer's policy", () => {
  // It reads every file of the build, which takes several seconds while the other packages' tests run beside it.
  it.skipIf(!existsSync(join(dist, "index.html")))(
    "has no inline code, no remote address and no data: address",
    () => {
      expect(checkDist(dist)).toEqual([]);
    },
    30_000,
  );

  it("notices an inline script (the check itself works)", () => {
    expect(checkDist(join(dist, "no-such-build"))).toEqual([expect.stringContaining("nothing was built")]);
  });
});
