import { expect, it } from "vitest";

import { loroToPortable } from "./page-doc";
import { JS_SNAPSHOT, buildSampleDoc, readFixtureBytes, samplePage, writeFixtureBytes } from "./testing/fixtures";

// `pnpm --filter @atomik-kb/core-client fixtures` (vitest.fixtures.config.ts, never part of
// `test`): writes `fixtures/doc/page-v1.from-js.snapshot.bin`, the Loro snapshot the
// TypeScript side of the portable conversion produces for `page-v1.portable.json` (fixed
// peer id, no timestamps, so the bytes are reproducible). kb-model reads it to check that
// a JS-written document decodes to the same portable JSON. Regenerate only when the
// document shape changes on purpose (new FORMAT_VERSION, ADR-0016).
it("writes page-v1.from-js.snapshot.bin", () => {
  const doc = buildSampleDoc();
  expect(loroToPortable(doc)).toEqual(samplePage());
  const snapshot = doc.export({ mode: "snapshot" });
  writeFixtureBytes(JS_SNAPSHOT, snapshot);
  expect(readFixtureBytes(JS_SNAPSHOT)).toEqual(snapshot);
});
