// The breadcrumb block (ADR-0016, amendment of 2026-10-06) and the chain component behind it:
// the schema against the fixture, the pure trail of a page, `/breadcrumb`, the chain for a nested
// page and for a page at the root, how it follows renames, icons and moves of the tree, a click
// on an ancestor, read-only editors, deleting the block, and that the view writes nothing.

import { CoreClient } from "@atomik-kb/core-client";
import type { JsonValue, PageInfo, SeedPage, SpaceInfo, TreeView } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { NodeSelection, TextSelection } from "@tiptap/pm/state";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import type { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";

import blockSchemaFixture from "../../../../fixtures/doc/block-schema-v1.json";
import { breadcrumbTrail } from "../components/Breadcrumbs";
import { t } from "../i18n";
import { pid } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { describeSchema } from "../test/schema-fixture";
import type { BlockSchemaFixture } from "../test/schema-fixture";
import { pathTo } from "../tree";
import { duplicateBlock, turnInto, turnIntoKinds } from "./blocks";
import { loroSyncPluginKey } from "./loro-binding";
import { createEditorSchema, ID_BEARING_NODES } from "./schema";
import { filterSlashCommands, SLASH_COMMANDS } from "./slash-commands";

afterEach(() => {
  vi.restoreAllMocks();
});

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

// ----- documents and trees -----------------------------------------------------------------

function crumbsBlock(n: number): JsonValue {
  return { type: "breadcrumb", attrs: { id: pid(n) } };
}

function paragraph(n: number, value: string): JsonValue {
  return { type: "paragraph", attrs: { id: pid(n) }, content: [{ type: "text", text: value }] };
}

/** A page that opens with a breadcrumb. */
function withBreadcrumb(): JsonValue {
  return { type: "doc", content: [crumbsBlock(1), paragraph(2, "Below the chain")] };
}

/**
 * Personal: Projects (📁) > atomik-kb (🧠) > Roadmap (🗺️, holds the breadcrumb), Design notes;
 * Journal; and a second space, Work.
 */
function tree(content: JsonValue = withBreadcrumb()): SeedPage[] {
  return [
    {
      title: "Projects",
      icon: "📁",
      children: [
        { title: "atomik-kb", icon: "🧠", children: [{ title: "Roadmap", icon: "🗺️", content }, { title: "Design notes" }] },
        { title: "Garden" },
      ],
    },
    { title: "Journal", content },
  ];
}

async function openRoadmap(content: JsonValue = withBreadcrumb()) {
  const app = await renderPages([
    { name: "Personal", icon: "🏠", pages: tree(content) },
    { name: "Work", pages: [{ title: "Meetings" }] },
  ]);
  const editor = await openPage(app.user, "Roadmap");
  return { ...app, editor, client: new CoreClient(app.memory) };
}

// ----- looking at the chain ----------------------------------------------------------------

function nav(): HTMLElement {
  return screen.getByRole("navigation", { name: t("breadcrumb.label") });
}

/** The titles of the chain, in order. */
function titles(): string[] {
  return Array.from(nav().querySelectorAll(".breadcrumb-title")).map((title) => title.textContent ?? "");
}

/** The icons of the pages of the chain, `""` for one that shows the default glyph. */
function icons(): string[] {
  return Array.from(nav().querySelectorAll(".breadcrumb-item")).map((item) => item.querySelector(".breadcrumb-icon")?.textContent ?? "");
}

/** The buttons of the chain, by title. */
function buttons(): string[] {
  return within(nav())
    .queryAllByRole("button")
    .map((button) => button.querySelector(".breadcrumb-title")?.textContent ?? "");
}

/** The title of the open page (its icon is not part of it). */
function pageTitle(): string {
  return screen.getByRole("heading", { level: 1 }).querySelector(".page-title-button")?.textContent ?? "";
}

function change(run: () => void): void {
  act(() => {
    run();
  });
}

// ----- the schema --------------------------------------------------------------------------

describe("the schema", () => {
  const fixture = blockSchemaFixture as unknown as BlockSchemaFixture;
  const live = describeSchema(createEditorSchema());

  it("has the breadcrumb of the fixture: a leaf atom block with a block id and nothing else", () => {
    expect(live.nodes.breadcrumb).toEqual({ content: "", group: "block", inline: false, atom: true, attrs: ["id"] });
    expect(fixture.nodes.breadcrumb).toEqual(live.nodes.breadcrumb);
    expect(fixture.idBearing).toContain("breadcrumb");
    expect(ID_BEARING_NODES).toContain("breadcrumb");
  });

  it("is a leaf and an atom, and HTML <div data-type=breadcrumb>", () => {
    const type = createEditorSchema().nodes.breadcrumb;
    expect(type).toBeDefined();
    const node = type?.create({ id: pid(9) });
    expect(node?.isLeaf).toBe(true);
    expect(node?.isAtom).toBe(true);
    expect(type?.spec.parseDOM).toMatchObject([{ tag: 'div[data-type="breadcrumb"]' }]);
  });
});

// ----- the trail ---------------------------------------------------------------------------

describe("pathTo and breadcrumbTrail", () => {
  const info = (id: string, title: string, extra: Partial<PageInfo> = {}): PageInfo => ({
    id,
    spaceId: "space-1",
    title,
    createdAt: 0,
    updatedAt: 0,
    ...extra,
  });
  const view: TreeView = {
    spaceId: "space-1",
    roots: [
      {
        page: info("a", "A", { icon: "🅰️" }),
        children: [{ page: info("b", "B"), children: [{ page: info("c", ""), children: [] }] }],
      },
      { page: info("d", "D"), children: [] },
    ],
    trashed: [],
    undo: { canUndo: false, canRedo: false },
  };
  const space = (id: string, name: string): SpaceInfo => ({ id, name, createdAt: 0, pageCount: 0 });
  const spaces = [space("space-0", "Other"), space("space-1", "Personal")];

  it("lists the pages from the top of the tree down to the page, itself last", () => {
    expect(pathTo(view.roots, "c")?.map((page) => page.id)).toEqual(["a", "b", "c"]);
    expect(pathTo(view.roots, "d")?.map((page) => page.id)).toEqual(["d"]);
    expect(pathTo(view.roots, "missing")).toBeNull();
    expect(pathTo([], "a")).toBeNull();
  });

  it("gives the space's name, the ancestors and the page", () => {
    const trail = breadcrumbTrail(view, spaces, "c");
    expect(trail?.space).toBe("Personal");
    expect(trail?.ancestors.map((page) => page.title)).toEqual(["A", "B"]);
    expect(trail?.page.id).toBe("c");
    expect(breadcrumbTrail(view, spaces, "d")?.ancestors).toEqual([]);
  });

  it("has no trail while the tree is not loaded or when it does not hold the page", () => {
    expect(breadcrumbTrail(null, spaces, "c")).toBeNull();
    expect(breadcrumbTrail(view, spaces, "elsewhere")).toBeNull();
  });

  it("takes an empty name when the space is not known (nothing to show for it)", () => {
    expect(breadcrumbTrail(view, [], "b")?.space).toBe("");
  });
});

// ----- the / menu --------------------------------------------------------------------------

describe("the / menu", () => {
  it("has the entry, found by its name and by breadcrumb, path, parents and ancestors", () => {
    const entry = SLASH_COMMANDS.find((command) => command.id === "breadcrumb");
    expect(entry).toBeDefined();
    expect(entry === undefined ? null : t(entry.label)).toBe("Breadcrumb");
    for (const word of ["bread", "path", "parents", "ancestors"]) {
      expect(filterSlashCommands(word).map((command) => command.id), word).toContain("breadcrumb");
    }
    expect(filterSlashCommands("toc").map((command) => command.id)).not.toContain("breadcrumb");
  });

  it("inserts the block where /breadcrumb was typed, with an id and a paragraph below it", async () => {
    const empty: JsonValue = { type: "paragraph", attrs: { id: pid(1) } };
    const { user, memory, editor } = await openRoadmap({ type: "doc", content: [paragraph(0, "Intro"), empty] });
    editor.view.dispatch(
      editor.state.tr.setSelection(TextSelection.create(editor.state.doc, editor.state.doc.child(0).nodeSize + 1)),
    );
    editor.commands.insertContent("/bread");

    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(within(menu).getAllByRole("option").map((option) => option.querySelector(".suggestion-title")?.textContent)).toEqual([
        "Breadcrumb",
      ]);
    });
    await user.click(within(menu).getByText("Breadcrumb"));

    expect(editor.state.doc.childCount).toBe(3);
    const block = editor.state.doc.child(1);
    const blockId: unknown = block.attrs.id;
    expect(block.type.name).toBe("breadcrumb");
    expect(blockId).toMatch(UUID);
    expect(editor.state.doc.child(2).type.name).toBe("paragraph");
    expect(editor.state.selection.$from.parent).toBe(editor.state.doc.child(2));
    // It draws the chain at once.
    expect(titles()).toEqual(["Personal", "Projects", "atomik-kb", "Roadmap"]);

    // What reaches the core is the block with its id, and nothing of the chain it shows.
    const { page } = await idsOf(memory);
    await waitFor(async () => {
      expect(JSON.stringify(await contentOf(memory, page("Roadmap")))).toContain('"breadcrumb"');
    });
    const stored = (await contentOf(memory, page("Roadmap"))) as { content: { type: string }[] };
    expect(stored.content.find((node) => node.type === "breadcrumb")).toEqual({
      type: "breadcrumb",
      attrs: { id: blockId },
    });
  });
});

// ----- the chain in the page editor --------------------------------------------------------

describe("the chain", () => {
  it("shows the space, each ancestor page with its icon and title, and the page itself", async () => {
    const { editor } = await openRoadmap();
    expect(titles()).toEqual(["Personal", "Projects", "atomik-kb", "Roadmap"]);
    expect(icons()[1]).toBe("📁");
    expect(icons()[2]).toBe("🧠");
    expect(icons()[3]).toBe("🗺️");
    // The ancestors are buttons; the space name and the page itself are not.
    expect(buttons()).toEqual(["Projects", "atomik-kb"]);
    const current = nav().querySelector("[aria-current='page']");
    expect(current?.querySelector(".breadcrumb-title")?.textContent).toBe("Roadmap");
    expect(within(current as HTMLElement).queryByRole("button")).not.toBeInTheDocument();
    // Chevrons separate the parts: three for four parts.
    expect(nav().querySelectorAll(".breadcrumb-separator")).toHaveLength(3);
    // Nothing of the chain is document text.
    expect(editor.getText()).not.toContain("Projects");
  });

  it("shows only the space name and the page for a page at the root", async () => {
    const { user, editor } = await openRoadmap();
    await user.click(screen.getByRole("treeitem", { name: "Journal" }));
    await waitFor(() => {
      expect(pageTitle()).toBe("Journal");
    });
    await waitFor(() => {
      expect(titles()).toEqual(["Personal", "Journal"]);
    });
    expect(buttons()).toEqual([]);
    expect(nav().querySelectorAll(".breadcrumb-separator")).toHaveLength(1);
    expect(editor.isDestroyed).toBe(true); // the page before has its own editor
  });

  it("shows an untitled page as Untitled", async () => {
    const { client, memory } = await openRoadmap();
    const { page } = await idsOf(memory);
    await client.pageRename(page("Roadmap"), "");
    await waitFor(() => {
      expect(titles()).toEqual(["Personal", "Projects", "atomik-kb", t("page.untitled")]);
    });
  });

  it("follows a rename of the page and of an ancestor", async () => {
    const { client, memory } = await openRoadmap();
    const { page } = await idsOf(memory);
    await client.pageRename(page("Projects"), "Work items");
    await waitFor(() => {
      expect(titles()).toEqual(["Personal", "Work items", "atomik-kb", "Roadmap"]);
    });
    await client.pageRename(page("Roadmap"), "Plan");
    await waitFor(() => {
      expect(titles()).toEqual(["Personal", "Work items", "atomik-kb", "Plan"]);
    });
    expect(buttons()).toEqual(["Work items", "atomik-kb"]);
  });

  it("follows a new icon, and the space's name", async () => {
    const { client, memory } = await openRoadmap();
    const { page, space } = await idsOf(memory);
    await client.pageSetIcon(page("atomik-kb"), "🚀");
    await waitFor(() => {
      expect(icons()[2]).toBe("🚀");
    });
    await client.spaceRename(space("Personal"), "Home");
    await waitFor(() => {
      expect(titles()[0]).toBe("Home");
    });
  });

  it("follows a move of the page and of an ancestor", async () => {
    const { client, memory } = await openRoadmap();
    const { page } = await idsOf(memory);
    // The page moves under Garden: the chain gets longer.
    await client.pageMove(page("Roadmap"), page("Garden"), 0);
    await waitFor(() => {
      expect(titles()).toEqual(["Personal", "Projects", "Garden", "Roadmap"]);
    });
    // An ancestor moves to the top: the chain loses its first parts.
    await client.pageMove(page("Garden"), null, 0);
    await waitFor(() => {
      expect(titles()).toEqual(["Personal", "Garden", "Roadmap"]);
    });
    // The page itself moves to the top.
    await client.pageMove(page("Roadmap"), null, 1);
    await waitFor(() => {
      expect(titles()).toEqual(["Personal", "Roadmap"]);
    });
    expect(buttons()).toEqual([]);
  });

  it("draws nothing for a page that has left the tree (moved to the trash)", async () => {
    const { client, memory } = await openRoadmap();
    const { page } = await idsOf(memory);
    await client.pageTrash(page("Roadmap"));
    await waitFor(() => {
      expect(screen.queryByRole("navigation", { name: t("breadcrumb.label") })).not.toBeInTheDocument();
    });
  });
});

// ----- clicking an ancestor ----------------------------------------------------------------

describe("a click on an ancestor", () => {
  it("opens that page", async () => {
    const { user } = await openRoadmap();
    await user.click(within(nav()).getByRole("button", { name: "atomik-kb" }));
    await waitFor(() => {
      expect(pageTitle()).toBe("atomik-kb");
    });
    // The page it opened has no breadcrumb block: its editor shows no chain.
    expect(screen.queryByRole("navigation", { name: t("breadcrumb.label") })).not.toBeInTheDocument();
  });

  it("opens the farthest ancestor, and the page can be reached again from the sidebar", async () => {
    const { user } = await openRoadmap();
    await user.click(within(nav()).getByRole("button", { name: "Projects" }));
    await waitFor(() => {
      expect(pageTitle()).toBe("Projects");
    });
    await user.click(screen.getByRole("treeitem", { name: "Roadmap" }));
    await waitFor(() => {
      expect(titles()).toEqual(["Personal", "Projects", "atomik-kb", "Roadmap"]);
    });
  });
});

// ----- read-only editors -------------------------------------------------------------------

describe("in a read-only editor", () => {
  it("shows the same chain, and its buttons still open the pages", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const content: JsonValue = {
      type: "doc",
      content: [crumbsBlock(1), paragraph(2, "Kept"), { type: "kanban", attrs: { id: "k0", columns: 3 } }],
    };
    const { user, memory, editor } = await openRoadmap(content);
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    expect(editor.isEditable).toBe(false);
    expect(titles()).toEqual(["Personal", "Projects", "atomik-kb", "Roadmap"]);

    await user.click(within(nav()).getByRole("button", { name: "Projects" }));
    await waitFor(() => {
      expect(pageTitle()).toBe("Projects");
    });
    const { page } = await idsOf(memory);
    expect(await contentOf(memory, page("Roadmap"))).toEqual(content);
  });
});

// ----- the block itself --------------------------------------------------------------------

describe("the block", () => {
  function selectFirst(editor: Editor): void {
    change(() => {
      editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, 0)));
    });
  }

  it("is selected as a whole, outlined, and deleted with Backspace or Delete", async () => {
    const { editor } = await openRoadmap();
    expect(editor.state.doc.child(0).type.name).toBe("breadcrumb");
    selectFirst(editor);
    const dom = editor.view.nodeDOM(0);
    expect(dom).toBeInstanceOf(HTMLElement);
    expect(dom).toHaveClass("ProseMirror-selectednode");
    expect(dom).toHaveAttribute("data-type", "breadcrumb");

    change(() => {
      expect(editor.commands.keyboardShortcut("Backspace")).toBe(true);
    });
    expect(JSON.stringify(editor.getJSON())).not.toContain("breadcrumb");
    expect(screen.queryByRole("navigation", { name: t("breadcrumb.label") })).not.toBeInTheDocument();

    change(() => {
      editor.commands.insertContentAt(0, { type: "breadcrumb" });
    });
    expect(editor.state.doc.child(0).type.name).toBe("breadcrumb");
    selectFirst(editor);
    change(() => {
      expect(editor.commands.keyboardShortcut("Delete")).toBe(true);
    });
    expect(JSON.stringify(editor.getJSON())).not.toContain("breadcrumb");
  });

  it("can be put twice on a page, and both show the chain", async () => {
    const content: JsonValue = { type: "doc", content: [crumbsBlock(1), paragraph(2, "x"), crumbsBlock(3)] };
    await openRoadmap(content);
    const chains = screen.getAllByRole("navigation", { name: t("breadcrumb.label") });
    expect(chains).toHaveLength(2);
    for (const chain of chains) {
      expect(Array.from(chain.querySelectorAll(".breadcrumb-title")).map((title) => title.textContent)).toEqual([
        "Personal",
        "Projects",
        "atomik-kb",
        "Roadmap",
      ]);
    }
  });
});

// ----- nothing is written ------------------------------------------------------------------

describe("the document", () => {
  it("records no change when the page is shown, the chain drawn, and the tree changes", async () => {
    const { editor, client, memory } = await openRoadmap();
    const doc = loroSyncPluginKey.getState(editor.state)?.doc as LoroDoc;
    // This replica wrote nothing: it only imported what the core held.
    const own = (): number => doc.oplogVersion().get(doc.peerIdStr) ?? 0;
    expect(own()).toBe(0);

    const { page } = await idsOf(memory);
    await client.pageRename(page("atomik-kb"), "Renamed");
    await waitFor(() => {
      expect(titles()).toContain("Renamed");
    });
    await client.pageMove(page("Roadmap"), null, 0);
    await waitFor(() => {
      expect(titles()).toEqual(["Personal", "Roadmap"]);
    });
    expect(own()).toBe(0);
    fireEvent.focus(editor.view.dom);
    expect(own()).toBe(0);
    expect(await contentOf(memory, page("Roadmap"))).toEqual(withBreadcrumb());
  });
});

describe("the block menu", () => {
  it("offers a quote or a callout around the block, never text that would replace it with an empty block", async () => {
    const { editor } = await openRoadmap();
    expect(turnIntoKinds(editor.state.doc.child(0))).toEqual(["blockquote", "callout"]);
    expect(turnInto(editor.state, 0, "heading2")).toBeNull();

    const wrap = turnInto(editor.state, 0, "blockquote");
    expect(wrap).not.toBeNull();
    change(() => {
      if (wrap !== null) {
        editor.view.dispatch(wrap);
      }
    });
    expect(editor.state.doc.child(0).type.name).toBe("blockquote");
    expect(editor.state.doc.child(0).child(0).type.name).toBe("breadcrumb");
    // Still the chain of the page, drawn from inside the quote (a new view: drawn a moment later).
    await waitFor(() => {
      expect(titles()).toEqual(["Personal", "Projects", "atomik-kb", "Roadmap"]);
    });
  });

  it("duplicates the block with an id of its own", async () => {
    const { editor } = await openRoadmap();
    const copy = duplicateBlock(editor.state, 0);
    expect(copy).not.toBeNull();
    change(() => {
      if (copy !== null) {
        editor.view.dispatch(copy);
      }
    });
    const ids = [editor.state.doc.child(0).attrs.id, editor.state.doc.child(1).attrs.id] as unknown[];
    expect(ids[0]).toBe(pid(1));
    expect(ids[1]).toMatch(UUID);
    expect(editor.state.doc.child(1).type.name).toBe("breadcrumb");
    await waitFor(() => {
      expect(screen.getAllByRole("navigation", { name: t("breadcrumb.label") })).toHaveLength(2);
    });
  });
});
