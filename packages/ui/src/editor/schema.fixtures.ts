// Run by `pnpm --filter @atomik-kb/ui fixtures` (vitest.fixtures.config.ts), never by `test`.

import { writeFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import { BLOCK_SCHEMA_FIXTURE, describeSchema, serializeFixture } from "../test/schema-fixture";
import { createEditorSchema } from "./schema";

describe("fixtures", () => {
  it("writes fixtures/doc/block-schema-v1.json from the live schema", () => {
    const text = serializeFixture(describeSchema(createEditorSchema()));
    writeFileSync(BLOCK_SCHEMA_FIXTURE, text);
    expect(JSON.parse(text)).toMatchObject({ format: 1 });
  });
});
