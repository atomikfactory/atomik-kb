// `remapView` (ADR-0038 step 5, extended by ADR-0054): the attributes of a `pageList` block, and every view in
// its `views`, with the ids of the source space replaced by the target's. The mirror in `memory-move.test.ts` moves
// real pages; these are the rules on plain JSON, with the golden examples of `docs/architecture/database-views.md`
// (section 2.2), which the Rust twin (`kb-core` `space_move/mapping.rs`) holds to the same results.

import { describe, expect, it } from "vitest";

import type { JsonValue } from "./generated/serde_json/JsonValue";
import { remapView } from "./memory-move-values";
import type { ViewMap } from "./memory-move-values";
import type { Attrs } from "./page-doc";

function uuid(n: number): string {
  return `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}

const STATUS = uuid(0x101);
const DUE = uuid(0x102);
const PRIORITY = uuid(0x103);
const END = uuid(0x104);
const GONE = uuid(0x1ff);
const TODO = uuid(0x201);
const HIGH = uuid(0x202);
const LOW = uuid(0x203);

const isSet = (propertyId: string): JsonValue => ({ propertyId, op: "isSet" });
const hasOption = (propertyId: string, value: string): JsonValue => ({ propertyId, op: "hasOption", value });

/** The block of the design document: the mirror of its first view, then four views of four layouts. */
function golden(): Attrs {
  return {
    id: "block",
    source: { kind: "children" },
    layout: "table",
    properties: [STATUS, DUE],
    sort: { by: DUE, dir: "asc" },
    filters: [hasOption(STATUS, TODO)],
    columnWidths: { title: 320 },
    views: [
      {
        id: "k3f9a2bc",
        name: "",
        layout: "table",
        properties: [STATUS, DUE],
        sort: { by: DUE, dir: "asc" },
        filter: {
          op: "and",
          items: [hasOption(STATUS, TODO), { op: "or", items: [hasOption(PRIORITY, HIGH), isSet(DUE)] }],
        },
        columnWidths: { title: 320 },
      },
      {
        id: "b81d00e4",
        name: "By status",
        layout: "board",
        properties: [DUE],
        sort: { by: "title", dir: "asc" },
        groupBy: STATUS,
        hiddenGroups: ["none"],
        openIn: "center",
      },
      { id: "7c0a55d1", name: "Schedule", layout: "calendar", properties: [STATUS], sort: { by: "title", dir: "asc" }, dateBy: DUE, endBy: END },
      { id: "e2a9b6f0", name: "", layout: "gallery", properties: [], sort: { by: "created", dir: "desc" }, cardSize: "l" },
    ],
  };
}

/** A map that sends every id to itself (and Low, which nothing in the golden example uses, to nothing). */
const identity: ViewMap = {
  property: (id) => (id === GONE ? undefined : id),
  option: (_, id) => (id === LOW ? undefined : id),
};

/** A map to the target's ids: `t:` before each; `Gone` and the option `Low` have no counterpart. */
const renaming: ViewMap = {
  property: (id) => (id === GONE ? undefined : `t:${id}`),
  option: (_, id) => (id === LOW ? undefined : `t:${id}`),
};

function viewsOf(attrs: Attrs): Record<string, JsonValue>[] {
  return attrs.views as Record<string, JsonValue>[];
}

describe("remapView of a block with views", () => {
  it("leaves the golden example as it is when every id maps to itself", () => {
    expect(remapView(golden(), identity)).toEqual(golden());
  });

  it("follows the mapping in the block and in every view, the mirror staying equal to the first view", () => {
    const out = remapView(golden(), renaming);
    const [first, board, calendar, gallery] = viewsOf(out);
    expect(out.properties).toEqual([`t:${STATUS}`, `t:${DUE}`]);
    expect(out.sort).toEqual({ by: `t:${DUE}`, dir: "asc" });
    expect(out.filters).toEqual([hasOption(`t:${STATUS}`, `t:${TODO}`)]);
    expect(first).toEqual({
      id: "k3f9a2bc",
      name: "",
      layout: "table",
      properties: out.properties,
      sort: out.sort,
      filter: {
        op: "and",
        items: [
          hasOption(`t:${STATUS}`, `t:${TODO}`),
          { op: "or", items: [hasOption(`t:${PRIORITY}`, `t:${HIGH}`), isSet(`t:${DUE}`)] },
        ],
      },
      columnWidths: { title: 320 },
    });
    expect(board).toMatchObject({ groupBy: `t:${STATUS}`, hiddenGroups: ["none"], properties: [`t:${DUE}`], openIn: "center", name: "By status" });
    expect(calendar).toMatchObject({ dateBy: `t:${DUE}`, endBy: `t:${END}`, properties: [`t:${STATUS}`] });
    // Nothing in a gallery names an id.
    expect(gallery).toEqual((golden().views as JsonValue[])[3]);
    // The block's own id and tag source are not a view's business.
    expect(out.id).toBe("block");
    expect(out.source).toEqual({ kind: "children" });
  });

  it("drops what has no counterpart: filters, a group left empty, a calendar's dates, the grouping", () => {
    const attrs: Attrs = {
      views: [
        {
          id: "a",
          name: "x",
          layout: "table",
          properties: [GONE, DUE],
          sort: { by: GONE, dir: "asc" },
          filter: {
            op: "or",
            items: [
              isSet(GONE),
              hasOption(PRIORITY, LOW),
              // A group that loses every filter goes, one that keeps one stays, one that was empty stays.
              { op: "and", items: [isSet(GONE), hasOption(PRIORITY, LOW)] },
              { op: "and", items: [isSet(GONE), isSet(DUE)] },
              { op: "and", items: [] },
            ],
          },
        },
        { id: "b", name: "", layout: "board", properties: [], sort: { by: "title", dir: "asc" }, groupBy: GONE, hiddenGroups: [HIGH, "none"] },
        { id: "c", name: "", layout: "calendar", properties: [], sort: { by: "title", dir: "asc" }, dateBy: GONE, endBy: GONE },
        { id: "d", name: "", layout: "table", properties: [], sort: { by: "title", dir: "asc" }, filter: { op: "or", items: [isSet(GONE)] } },
      ],
    };
    const [a, b, c, d] = viewsOf(remapView(attrs, renaming));
    expect(a).toEqual({
      id: "a",
      name: "x",
      layout: "table",
      properties: [`t:${DUE}`],
      filter: { op: "or", items: [{ op: "and", items: [isSet(`t:${DUE}`)] }, { op: "and", items: [] }] },
    });
    expect(b).toEqual({ id: "b", name: "", layout: "board", properties: [], sort: { by: "title", dir: "asc" } });
    expect(c).toEqual({ id: "c", name: "", layout: "calendar", properties: [], sort: { by: "title", dir: "asc" } });
    expect(d).toEqual({ id: "d", name: "", layout: "table", properties: [], sort: { by: "title", dir: "asc" } });
  });

  it("keeps `created`, `updated`, a date that is no id, the keys it does not know and entries that are no view", () => {
    const attrs: Attrs = {
      views: [
        { id: "a", name: "", layout: "calendar", properties: [], sort: { by: "updated", dir: "desc" }, dateBy: "created", endBy: "rollup:later", futureKey: { ids: [STATUS] }, cardSize: "xl" },
        { id: "b", name: "", layout: "calendar", properties: [], dateBy: "updated" },
        "not a view",
        7,
        null,
        ["nested"],
      ],
    };
    expect(remapView(attrs, renaming)).toEqual(attrs);
  });

  it("is what the block does alone when it has no `views`, and ignores a `views` that is no list", () => {
    const legacy: Attrs = {
      layout: "board",
      properties: [STATUS, GONE],
      sort: { by: STATUS, dir: "desc" },
      filters: [hasOption(STATUS, TODO), isSet(GONE)],
      groupBy: STATUS,
    };
    const out = remapView(legacy, renaming);
    expect(out).toEqual({
      layout: "board",
      properties: [`t:${STATUS}`],
      sort: { by: `t:${STATUS}`, dir: "desc" },
      filters: [hasOption(`t:${STATUS}`, `t:${TODO}`)],
      groupBy: `t:${STATUS}`,
    });
    expect("views" in out).toBe(false);
    expect(remapView({ ...legacy, views: "nope" }, renaming).views).toBe("nope");
  });

  it("never throws on a stored view of any shape", () => {
    let deep: JsonValue = { op: "and", items: [] };
    for (let level = 0; level < 300; level += 1) {
      deep = { op: "or", items: [deep, isSet(DUE)] };
    }
    const garbage: JsonValue[] = [
      {},
      { id: 4, properties: "x", sort: [], filter: { items: 3 }, columnWidths: [], calculations: 1, groupBy: 9, hiddenGroups: "x", dateBy: {}, endBy: [] },
      { id: "deep", filter: deep },
      { id: "odd", filter: { op: "or", items: [null, 3, "x", [], { op: "or", items: "no" }, { propertyId: 5 }] } },
    ];
    expect(() => remapView({ views: garbage, filter: deep, filters: 4, dateBy: 1 }, renaming)).not.toThrow();
  });
});

describe("remapView of the parts of ADR-0058", () => {
  const OTHER = uuid(0x301);
  const lacksOption = (propertyId: string, value: JsonValue): JsonValue => ({ propertyId, op: "lacksOption", value });
  const sortsOf = (view: Record<string, JsonValue> | undefined): JsonValue | undefined => view?.sorts;

  it("follows each sort key like `sort.by`, drops the keys with no counterpart and keeps `sort` the first key left", () => {
    const view = (sort: JsonValue, sorts: JsonValue): Attrs => ({ views: [{ id: "v", name: "", layout: "table", properties: [DUE], sort, sorts }] });
    const first = (attrs: Attrs): Record<string, JsonValue> | undefined => viewsOf(attrs)[0];

    // Every key has a counterpart (or is a built-in): each `by` follows, with the keys it does not know.
    let out = first(
      remapView(
        view({ by: DUE, dir: "desc", x: 1 }, [
          { by: DUE, dir: "desc", x: 1 },
          { by: "created", dir: "asc" },
          { by: PRIORITY, dir: "desc" },
          { by: "title", dir: "asc" },
        ]),
        renaming,
      ),
    );
    expect(sortsOf(out)).toEqual([
      { by: `t:${DUE}`, dir: "desc", x: 1 },
      { by: "created", dir: "asc" },
      { by: `t:${PRIORITY}`, dir: "desc" },
      { by: "title", dir: "asc" },
    ]);
    expect(out?.sort).toEqual({ by: `t:${DUE}`, dir: "desc", x: 1 });

    // A key with no counterpart goes; `sort` stays the first.
    out = first(
      remapView(
        view({ by: DUE, dir: "asc" }, [
          { by: DUE, dir: "asc" },
          { by: GONE, dir: "desc" },
          { by: "updated", dir: "desc" },
        ]),
        renaming,
      ),
    );
    expect(sortsOf(out)).toEqual([
      { by: `t:${DUE}`, dir: "asc" },
      { by: "updated", dir: "desc" },
    ]);
    expect(out?.sort).toEqual({ by: `t:${DUE}`, dir: "asc" });

    // The first key goes: `sort` follows the first one left.
    out = first(
      remapView(
        view({ by: GONE, dir: "asc" }, [
          { by: GONE, dir: "asc" },
          { by: PRIORITY, dir: "desc" },
          { by: "created", dir: "asc" },
        ]),
        renaming,
      ),
    );
    expect(sortsOf(out)).toEqual([
      { by: `t:${PRIORITY}`, dir: "desc" },
      { by: "created", dir: "asc" },
    ]);
    expect(out?.sort).toEqual({ by: `t:${PRIORITY}`, dir: "desc" });

    // Every key goes: both go.
    out = first(
      remapView(
        view({ by: GONE, dir: "asc" }, [
          { by: GONE, dir: "asc" },
          { by: GONE, dir: "desc" },
        ]),
        renaming,
      ),
    );
    expect(out).toEqual({ id: "v", name: "", layout: "table", properties: [`t:${DUE}`] });
  });

  it("keeps what it does not understand in `sorts`, and gives a view with no `sort` none", () => {
    const [view] = viewsOf(
      remapView({ views: [{ id: "v", sorts: ["junk", { dir: "asc" }, { by: GONE, dir: "asc" }, { by: DUE, dir: "asc" }] }] }, renaming),
    );
    expect(sortsOf(view)).toEqual(["junk", { dir: "asc" }, { by: `t:${DUE}`, dir: "asc" }]);
    expect(view).not.toHaveProperty("sort");
    expect(sortsOf(viewsOf(remapView({ views: [{ id: "v", sorts: [] }] }, renaming))[0])).toEqual([]);
    expect(sortsOf(viewsOf(remapView({ views: [{ id: "v", sorts: "title" }] }, renaming))[0])).toBe("title");
  });

  it("follows `lacksOption` like `hasOption` (one id or a list), keeps relative values and page ids as they are", () => {
    const [view] = viewsOf(
      remapView(
        {
          views: [
            {
              id: "v",
              filter: {
                op: "and",
                items: [
                  lacksOption(STATUS, TODO),
                  lacksOption(STATUS, LOW),
                  lacksOption(STATUS, [TODO, LOW]),
                  lacksOption(STATUS, [LOW]),
                  lacksOption(GONE, TODO),
                  hasOption(STATUS, TODO),
                  { propertyId: DUE, op: "notRelatesTo", value: OTHER },
                  { propertyId: DUE, op: "within", value: { relative: "nextDays", n: 3 } },
                  { propertyId: DUE, op: "onOrAfter", value: { relative: "yesterday" } },
                  { propertyId: DUE, op: "notEquals", value: 4 },
                ],
              },
            },
          ],
        },
        renaming,
      ),
    );
    expect(view?.filter).toEqual({
      op: "and",
      items: [
        lacksOption(`t:${STATUS}`, `t:${TODO}`),
        lacksOption(`t:${STATUS}`, [`t:${TODO}`]),
        hasOption(`t:${STATUS}`, `t:${TODO}`),
        { propertyId: `t:${DUE}`, op: "notRelatesTo", value: OTHER },
        { propertyId: `t:${DUE}`, op: "within", value: { relative: "nextDays", n: 3 } },
        { propertyId: `t:${DUE}`, op: "onOrAfter", value: { relative: "yesterday" } },
        { propertyId: `t:${DUE}`, op: "notEquals", value: 4 },
      ],
    });
  });

  it("keeps a `page` or `space` source as it is while the ids of the views follow", () => {
    const sources: JsonValue[] = [{ kind: "page", pageId: OTHER }, { kind: "space" }];
    for (const source of sources) {
      const out = remapView({ source, properties: [DUE], views: [{ id: "v", properties: [DUE] }] }, renaming);
      expect(out.source).toEqual(source);
      expect(out.properties).toEqual([`t:${DUE}`]);
      expect(viewsOf(out)[0]?.properties).toEqual([`t:${DUE}`]);
    }
  });
});
