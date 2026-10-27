// Block schema v1 parity: the live Tiptap schema against `fixtures/doc/block-schema-v1.json`,
// which the Rust side compares with `kb-model`. Names and mark expansion are normative
// (ADR-0016); attribute lists follow Tiptap, so a change to them means regenerating the
// fixture (`pnpm --filter @atomik-kb/ui fixtures`) and letting the Rust parity test judge.

import { describe, expect, it } from "vitest";

import blockSchemaFixture from "../../../../fixtures/doc/block-schema-v1.json";
import { describeSchema } from "../test/schema-fixture";
import type { BlockSchemaFixture } from "../test/schema-fixture";
import { createEditorSchema } from "./schema";

const fixture = blockSchemaFixture as unknown as BlockSchemaFixture;
const live = describeSchema(createEditorSchema());

function expansion(shape: BlockSchemaFixture): Record<string, string> {
  return Object.fromEntries(Object.entries(shape.marks).map(([name, mark]) => [name, mark.expand]));
}

describe("block schema v1 parity", () => {
  it("has the node and mark names of the fixture, and the same id-bearing nodes", () => {
    expect(Object.keys(live.nodes).sort()).toEqual(Object.keys(fixture.nodes).sort());
    expect(Object.keys(live.marks).sort()).toEqual(Object.keys(fixture.marks).sort());
    expect([...live.idBearing].sort()).toEqual([...fixture.idBearing].sort());
  });

  it("expands marks as the fixture says", () => {
    expect(expansion(live)).toEqual(expansion(fixture));
  });

  it("matches the fixture in every detail (else regenerate it with the fixtures script)", () => {
    expect(live).toEqual(fixture);
  });
});
