// `fixtures/doc/block-schema-v1.json`: the shape of block schema v1 as the live Tiptap
// schema defines it. The fixtures script writes it (`schema.fixtures.ts`), a test checks
// that the live schema still matches it, and a Rust test compares it with `kb-model`.

import type { Schema } from "@tiptap/pm/model";

import { getLoroTextStyle } from "../editor/loro-binding";
import { ID_BEARING_NODES } from "../editor/schema";

export interface NodeShape {
  content: string;
  group: string | null;
  inline: boolean;
  atom: boolean;
  attrs: string[];
}

export interface MarkShape {
  attrs: string[];
  expand: string;
}

export interface BlockSchemaFixture {
  format: number;
  note: string;
  nodes: Record<string, NodeShape>;
  marks: Record<string, MarkShape>;
  idBearing: string[];
}

/** Where the fixture lives, relative to this file. */
export const BLOCK_SCHEMA_FIXTURE = new URL("../../../../fixtures/doc/block-schema-v1.json", import.meta.url);

const NOTE =
  "Generated from the live Tiptap schema by `pnpm --filter @atomik-kb/ui fixtures` (packages/ui/src/editor/schema.fixtures.ts). Names and mark expansion are normative (ADR-0016); attribute lists follow Tiptap.";

/** The order of the spec table; nodes outside it (the attachment blocks of Milestone 5) follow by name. */
const NODE_ORDER = [
  "doc",
  "paragraph",
  "heading",
  "bulletList",
  "orderedList",
  "listItem",
  "taskList",
  "taskItem",
  "blockquote",
  "codeBlock",
  "horizontalRule",
  "hardBreak",
  "details",
  "detailsSummary",
  "detailsContent",
  "callout",
  "pageRef",
  "text",
];

const MARK_ORDER = ["bold", "italic", "underline", "strike", "code", "link", "textColor", "highlight"];

function ordered(names: readonly string[], order: readonly string[]): string[] {
  const known = order.filter((name) => names.includes(name));
  const rest = names.filter((name) => !order.includes(name)).sort();
  return [...known, ...rest];
}

/** The fixture's description of `schema`. */
export function describeSchema(schema: Schema): BlockSchemaFixture {
  const expand = getLoroTextStyle(schema);
  const nodes: Record<string, NodeShape> = {};
  for (const name of ordered(Object.keys(schema.nodes), NODE_ORDER)) {
    const type = schema.nodes[name];
    if (type === undefined) {
      continue;
    }
    nodes[name] = {
      content: type.spec.content ?? "",
      group: type.spec.group ?? null,
      inline: type.isInline,
      atom: type.spec.atom === true,
      attrs: Object.keys(type.spec.attrs ?? {}),
    };
  }
  const marks: Record<string, MarkShape> = {};
  for (const name of ordered(Object.keys(schema.marks), MARK_ORDER)) {
    const type = schema.marks[name];
    if (type === undefined) {
      continue;
    }
    marks[name] = { attrs: Object.keys(type.spec.attrs ?? {}), expand: expand[name]?.expand ?? "none" };
  }
  return { format: 1, note: NOTE, nodes, marks, idBearing: [...ID_BEARING_NODES] };
}

/** Writes the fixture in the layout of the hand-written original: one line per node and mark. */
export function serializeFixture(fixture: BlockSchemaFixture): string {
  const json = (value: unknown): string => JSON.stringify(value);
  const entry = (shape: object): string =>
    `{ ${Object.entries(shape)
      .map(([key, value]) => `${json(key)}: ${json(value).replace(/,(?=")/g, ", ")}`)
      .join(", ")} }`;
  const lines: string[] = ["{", `  "format": ${json(fixture.format)},`, `  "note": ${json(fixture.note)},`];
  lines.push('  "nodes": {');
  const nodeNames = Object.keys(fixture.nodes);
  nodeNames.forEach((name, index) => {
    const comma = index === nodeNames.length - 1 ? "" : ",";
    lines.push(`    ${json(name)}: ${entry(fixture.nodes[name] ?? {})}${comma}`);
  });
  lines.push("  },");
  lines.push('  "marks": {');
  const markNames = Object.keys(fixture.marks);
  markNames.forEach((name, index) => {
    const comma = index === markNames.length - 1 ? "" : ",";
    lines.push(`    ${json(name)}: ${entry(fixture.marks[name] ?? {})}${comma}`);
  });
  lines.push("  },");
  lines.push('  "idBearing": [');
  const ids = fixture.idBearing.map((name) => json(name));
  let line = "   ";
  const wrapped: string[] = [];
  for (const [index, id] of ids.entries()) {
    const piece = ` ${id}${index === ids.length - 1 ? "" : ","}`;
    if (line.length + piece.length > 100) {
      wrapped.push(line);
      line = "   ";
    }
    line += piece;
  }
  wrapped.push(line);
  lines.push(...wrapped);
  lines.push("  ]");
  lines.push("}");
  return `${lines.join("\n")}\n`;
}
