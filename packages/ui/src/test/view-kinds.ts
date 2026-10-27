// A space for the tests of the views (list, table, board) with the property kinds of ADR-0026
// (amendment of 2026-10-08) and ADR-0027: a place, the created / last edited time and by, a
// relation and a rollup over it. Hub holds the view; its children are Cafe, Zoo, Plain, Write and
// Ship (title order: Cafe, Plain, Ship, Write, Zoo).

import type { JsonValue, SeedSpace } from "@atomik-kb/core-client";

import { pid } from "./editor";

export const K = {
  where: "de000000-0000-4000-8000-00000000f001",
  created: "de000000-0000-4000-8000-00000000f002",
  edited: "de000000-0000-4000-8000-00000000f003",
  madeBy: "de000000-0000-4000-8000-00000000f004",
  editedBy: "de000000-0000-4000-8000-00000000f005",
  cost: "de000000-0000-4000-8000-00000000f006",
  tasks: "de000000-0000-4000-8000-00000000f007",
  total: "de000000-0000-4000-8000-00000000f008",
  titles: "de000000-0000-4000-8000-00000000f009",
  stage: "de000000-0000-4000-8000-00000000f00a",
};

/** Every kind of the amendments as a column, in this order. */
export const KIND_COLUMNS = [K.where, K.created, K.edited, K.madeBy, K.editedBy, K.tasks, K.total, K.titles];

/** A hub whose view has `attrs` (a table unless it says otherwise) and the kind columns. */
export function kindsHub(attrs: { [key: string]: JsonValue } = {}): JsonValue {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(1) }, content: [{ type: "text", text: "Intro" }] },
      {
        type: "pageList",
        attrs: {
          id: pid(2),
          source: { kind: "children" },
          layout: "table",
          properties: KIND_COLUMNS,
          sort: { by: "title", dir: "asc" },
          ...attrs,
        },
      },
      { type: "paragraph", attrs: { id: pid(3) } },
    ],
  };
}

export function kindsSpace(content: JsonValue = kindsHub()): SeedSpace {
  return {
    name: "Personal",
    properties: [
      { id: K.where, name: "Where", kind: "place" },
      { id: K.created, name: "Created", kind: "created_time" },
      { id: K.edited, name: "Edited", kind: "last_edited_time" },
      { id: K.madeBy, name: "Made by", kind: "created_by" },
      { id: K.editedBy, name: "Edited by", kind: "last_edited_by" },
      { id: K.cost, name: "Cost", kind: "number" },
      { id: K.tasks, name: "Tasks", kind: "relation" },
      { id: K.total, name: "Total", kind: "rollup", rollup: { relation: "Tasks", target: "Cost", function: "sum" } },
      { id: K.titles, name: "Names", kind: "rollup", rollup: { relation: "Tasks", target: "title", function: "show_original" } },
      {
        id: K.stage,
        name: "Stage",
        kind: "select",
        options: [
          { name: "Review", color: "orange" },
          { name: "Draft", color: "gray" },
        ],
      },
    ],
    pages: [
      {
        title: "Hub",
        content,
        children: [
          { title: "Cafe", properties: { Where: { name: "Café de Flore", lat: 48.854, lon: 2.333 }, Tasks: ["Write"], Stage: "Draft" } },
          { title: "Zoo", properties: { Where: { name: "Berlin Zoo" }, Tasks: ["Ship", "Write"], Stage: "Review" } },
          { title: "Plain" },
          { title: "Write", properties: { Cost: 3, Where: { name: "Attic" } } },
          { title: "Ship", properties: { Cost: 4.5 } },
        ],
      },
    ],
  };
}
