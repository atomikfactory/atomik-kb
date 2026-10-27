// The whiteboard block (ADR-0033, ADR-0016 amendment of 2026-10-10): the node in HTML, `/whiteboard`,
// the picture, the full-screen editor and its two-way sync with the page document (Excalidraw replaced by
// a stand-in), read-only editors, the drawing following a duplicated or pasted block, page undo not
// touching the drawing. Whole-app tests over the in-memory core, like gallery.test.tsx.

import { writeWhiteboardElements } from "@atomik-kb/core-client";
import type { JsonValue, MemoryTransport, WhiteboardValue } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { getSchema } from "@tiptap/core";
import { DOMParser as PmDOMParser, DOMSerializer, Slice } from "@tiptap/pm/model";
import { NodeSelection } from "@tiptap/pm/state";
import { act, screen, waitFor, within } from "@testing-library/react";
import type { ExcalidrawProps } from "@excalidraw/excalidraw/types";
import { useEffect } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { t } from "../i18n";
import { pid } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { duplicateBlock } from "./blocks";
import { setExcalidrawEngine } from "./excalidraw-engine";
import type { ExcalidrawEngine, SvgExport } from "./excalidraw-engine";
import { redo, undo } from "./loro-binding";
import { createSchemaExtensions } from "./schema";
import { clearWhiteboardStash, whiteboardCopyState, withNewWhiteboardIds } from "./whiteboard-copy";
import { readWhiteboardHeight } from "./whiteboard-nodes";
import type { WhiteboardStore } from "./whiteboard-store";
import { WRITE_DELAY_MS } from "./whiteboard-sync";

const BOARD = pid(1);

function el(id: string, version = 1, extra: WhiteboardValue = {}): WhiteboardValue {
  return {
    id,
    type: "rectangle",
    index: `a${id}`,
    version,
    versionNonce: 100,
    isDeleted: false,
    x: 1.5,
    y: 2,
    width: 40,
    height: 20,
    link: null,
    ...extra,
  };
}

function text(id: string, value: string, extra: WhiteboardValue = {}): WhiteboardValue {
  return el(id, 1, { type: "text", text: value, originalText: value, ...extra });
}

/** A stand-in for the library: records what the views ask of it and lets a test play the editor. */
function standIn() {
  const exported: SvgExport[] = [];
  const updates: { elements: readonly unknown[]; captureUpdate: unknown }[] = [];
  const state = { scene: [] as WhiteboardValue[], props: null as ExcalidrawProps | null, mounted: 0 };
  const api = {
    getSceneElementsIncludingDeleted: () => state.scene,
    updateScene: (data: { elements: readonly unknown[]; captureUpdate: unknown }) => {
      updates.push(data);
      state.scene = [...(data.elements as WhiteboardValue[])];
    },
  };
  const Excalidraw = (props: ExcalidrawProps) => {
    state.props = props;
    useEffect(() => {
      state.mounted += 1;
      const initial = props.initialData;
      state.scene = initial !== undefined && initial !== null && !(initial instanceof Promise) && typeof initial !== "function" ? [...((initial.elements ?? []) as unknown as WhiteboardValue[])] : [];
      props.excalidrawAPI?.(api as never);
      return () => {
        state.mounted -= 1;
      };
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);
    return <div data-testid="stand-in" data-view-mode={String(props.viewModeEnabled === true)} data-theme={props.theme} />;
  };
  const MainMenu = Object.assign(() => null, { DefaultItems: { ClearCanvas: () => null, ChangeCanvasBackground: () => null } });
  const engine = {
    Excalidraw,
    MainMenu,
    exportToSvg: (options: SvgExport) => {
      exported.push(options);
      const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      svg.setAttribute("data-count", String(options.elements.length));
      return Promise.resolve(svg);
    },
    restoreElements: (elements: readonly unknown[]) => elements,
    neverCapture: "never",
  } as unknown as ExcalidrawEngine;
  return {
    engine,
    exported,
    updates,
    state,
    /** The editor reports its scene, as Excalidraw's `onChange` does. */
    change(elements: WhiteboardValue[]) {
      state.scene = elements;
      act(() => {
        state.props?.onChange?.(elements as never, {} as never, {});
      });
    },
  };
}

beforeEach(() => {
  clearWhiteboardStash();
});

afterEach(() => {
  setExcalidrawEngine(null);
  vi.restoreAllMocks();
});

/** A page of a paragraph and a whiteboard (block `pid(1)`), the drawing as the portable `whiteboards` value. */
function pageWith(elements: WhiteboardValue[], height = 300): JsonValue {
  const page: { [key: string]: JsonValue } = {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Plan:" }] },
      { type: "whiteboard", attrs: { id: BOARD, height } },
    ],
  };
  if (elements.length > 0) {
    const drawing: { [id: string]: JsonValue } = {};
    for (const element of elements) {
      drawing[element.id as string] = element;
    }
    page.whiteboards = { [BOARD]: drawing };
  }
  return page;
}

const space = (content: JsonValue) => [{ name: "Personal", pages: [{ title: "Notes", content }] }];

async function open(elements: WhiteboardValue[], height?: number) {
  const library = standIn();
  setExcalidrawEngine(library.engine);
  const app = await renderPages(space(pageWith(elements, height)));
  const editor = await openPage(app.user, "Notes");
  return { ...app, editor, library };
}

function storeOf(editor: Editor): WhiteboardStore {
  const extension = editor.extensionManager.extensions.find((candidate) => candidate.name === "whiteboard");
  const store = (extension?.options as { store: WhiteboardStore | null } | undefined)?.store;
  if (store === undefined || store === null) {
    throw new Error("no whiteboard store");
  }
  return store;
}

async function stored(memory: MemoryTransport): Promise<{ [id: string]: WhiteboardValue }> {
  const { page } = await idsOf(memory);
  const content = (await contentOf(memory, page("Notes"))) as { whiteboards?: { [block: string]: { [id: string]: WhiteboardValue } } };
  return content.whiteboards?.[BOARD] ?? {};
}

function blockElement(editor: Editor): HTMLElement {
  const found = editor.view.dom.querySelector(".whiteboard-block");
  if (!(found instanceof HTMLElement)) {
    throw new Error("no whiteboard in the page");
  }
  return found;
}

function whiteboardNodes(editor: Editor): { id: string; pos: number }[] {
  const found: { id: string; pos: number }[] = [];
  editor.state.doc.descendants((node, pos) => {
    if (node.type.name === "whiteboard") {
      found.push({ id: String(node.attrs.id), pos });
    }
    return true;
  });
  return found;
}

describe("the node", () => {
  it("is a leaf block with a height, read leniently", () => {
    const schema = getSchema(createSchemaExtensions());
    const node = schema.nodes.whiteboard;
    expect(node?.isLeaf).toBe(true);
    expect(node?.spec.group).toBe("block");
    expect(Object.keys(node?.spec.attrs ?? {})).toEqual(["id", "height"]);
    expect(readWhiteboardHeight(undefined)).toBe(360);
    expect(readWhiteboardHeight(160)).toBe(160);
    expect(readWhiteboardHeight("1200")).toBe(1200);
    expect(readWhiteboardHeight(159)).toBe(360);
    expect(readWhiteboardHeight(1201)).toBe(360);
    expect(readWhiteboardHeight(300.5)).toBe(360);
  });

  it("round-trips through HTML with its height", () => {
    const schema = getSchema(createSchemaExtensions());
    const made = schema.nodes.whiteboard?.create({ id: "w1", height: 500 });
    expect(made).toBeDefined();
    const holder = document.createElement("div");
    holder.append(DOMSerializer.fromSchema(schema).serializeNode(made ?? schema.nodes.paragraph!.create()));
    expect(holder.innerHTML).toContain('data-type="whiteboard"');
    expect(holder.innerHTML).toContain('data-height="500"');
    const parsed = PmDOMParser.fromSchema(schema).parse(holder);
    expect(parsed.firstChild?.type.name).toBe("whiteboard");
    expect(parsed.firstChild?.attrs.height).toBe(500);
  });
});

describe("/whiteboard", () => {
  it("inserts an empty whiteboard that shows its hint and loads no library", async () => {
    const library = standIn();
    setExcalidrawEngine(library.engine);
    const { user, editor } = await (async () => {
      const app = await renderPages(space({ type: "doc", content: [{ type: "paragraph", attrs: { id: pid(0) } }] }));
      return { ...app, editor: await openPage(app.user, "Notes") };
    })();
    editor.commands.focus("end");
    editor.commands.insertContent("/whiteboard");
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await user.click(within(menu).getByText(t("slash.whiteboard")));
    await waitFor(() => {
      expect(whiteboardNodes(editor)).toHaveLength(1);
    });
    const node = editor.state.doc.nodeAt(whiteboardNodes(editor)[0]?.pos ?? 0);
    expect(node?.attrs.height).toBe(360);
    expect(typeof node?.attrs.id).toBe("string");
    expect(within(blockElement(editor)).getByText(t("whiteboard.empty"))).toBeInTheDocument();
    expect(library.exported).toEqual([]);
    expect(library.state.mounted).toBe(0);
    // `/drawing` finds it too.
    editor.commands.focus("end");
    editor.commands.insertContent("/drawing");
    expect(await screen.findByRole("option", { name: new RegExp(t("slash.whiteboard"), "u") })).toBeInTheDocument();
  });
});

describe("the picture", () => {
  it("draws the live elements without their links or images, at the block's height", async () => {
    const { editor, library } = await open(
      [
        el("r1", 1, { link: "https://example.org/" }),
        text("t1", "Hello"),
        el("gone", 3, { isDeleted: true }),
        el("img", 1, { type: "image", fileId: "f" }),
        el("emb", 1, { type: "embeddable" }),
      ],
      240,
    );
    await waitFor(() => {
      expect(blockElement(editor).querySelector(".whiteboard-svg > svg")).not.toBeNull();
    });
    const drawn = library.exported[0]?.elements as WhiteboardValue[];
    expect(drawn.map((e) => e.id)).toEqual(["r1", "t1"]);
    expect(drawn.every((e) => e.link === null)).toBe(true);
    expect(library.exported[0]?.appState.exportBackground).toBe(false);
    expect(blockElement(editor).querySelector<HTMLElement>(".whiteboard-stage")?.style.height).toBe("240px");
  });

  it("follows the drawing when it changes elsewhere", async () => {
    const { editor, memory, library } = await open([el("r1")]);
    await waitFor(() => {
      expect(blockElement(editor).querySelector(".whiteboard-svg > svg")?.getAttribute("data-count")).toBe("1");
    });
    const { page } = await idsOf(memory);
    act(() => {
      memory.changePage(
        page("Notes"),
        (doc) => {
          writeWhiteboardElements(doc, BOARD, [el("r2")]);
        },
        "Core",
      );
    });
    await waitFor(() => {
      expect(blockElement(editor).querySelector(".whiteboard-svg > svg")?.getAttribute("data-count")).toBe("2");
    });
    expect(library.exported.length).toBeGreaterThanOrEqual(2);
  });

  it("does not rebuild the page for a change of the drawing alone", async () => {
    const { editor, memory } = await open([el("r1")]);
    const dispatched = vi.spyOn(editor.view, "dispatch");
    const { page } = await idsOf(memory);
    act(() => {
      memory.changePage(
        page("Notes"),
        (doc) => {
          writeWhiteboardElements(doc, BOARD, [el("r2")]);
        },
        "Core",
      );
    });
    await waitFor(() => {
      expect(storeOf(editor).read(BOARD)).toHaveLength(2);
    });
    expect(dispatched).not.toHaveBeenCalled();
  });
});

describe("the full-screen editor", () => {
  it("opens from the Edit button with the document's elements, tombstones included, and closes with Done", async () => {
    const { user, editor, library } = await open([el("r1"), el("gone", 3, { isDeleted: true }), el("img", 1, { type: "image" })]);
    await user.click(within(blockElement(editor)).getByRole("button", { name: t("whiteboard.edit") }));
    const dialog = await screen.findByRole("dialog", { name: t("whiteboard.title") });
    expect(await within(dialog).findByTestId("stand-in")).toHaveAttribute("data-view-mode", "false");
    const initial = library.state.props?.initialData as unknown as { elements: WhiteboardValue[] };
    expect(initial.elements.map((e) => e.id)).toEqual(["gone", "r1"]); // z-order: the tombstone stays in the scene
    const props = library.state.props;
    // Nothing that reaches another host is on offer.
    expect(props?.UIOptions?.tools?.image).toBe(false);
    expect(props?.UIOptions?.canvasActions).toMatchObject({ loadScene: false, export: false, saveAsImage: false });
    expect(props?.aiEnabled).toBe(false);
    expect(props?.validateEmbeddable).toBe(false);
    expect(props?.handleKeyboardGlobally).toBe(false);
    const prevented = vi.fn();
    props?.onLinkOpen?.({} as never, { preventDefault: prevented } as never);
    expect(prevented).toHaveBeenCalled();
    expect(props?.onPaste?.({}, { clipboardData: { files: [new File(["x"], "a.png")] } } as never)).toBe(false);
    expect(props?.onPaste?.({}, { clipboardData: { files: [] } } as never)).toBe(true);
    await user.click(within(dialog).getByRole("button", { name: t("whiteboard.close") }));
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: t("whiteboard.title") })).not.toBeInTheDocument();
    });
    expect(library.state.mounted).toBe(0);
  });

  it("opens on a double click of the picture", async () => {
    const { user, editor } = await open([el("r1")]);
    await user.dblClick(blockElement(editor).querySelector(".whiteboard-stage") ?? blockElement(editor));
    expect(await screen.findByRole("dialog", { name: t("whiteboard.title") })).toBeInTheDocument();
  });

  it("writes only the elements that changed, one commit after a pause, and not the images", async () => {
    const { user, editor, memory, library } = await open([el("r1"), el("r2")]);
    const writes = vi.spyOn(storeOf(editor), "write");
    await user.click(within(blockElement(editor)).getByRole("button", { name: t("whiteboard.edit") }));
    await screen.findByTestId("stand-in");
    library.change([el("r1"), el("r2", 2, { x: 99 }), el("r3"), el("pic", 1, { type: "image" })]);
    expect(writes).not.toHaveBeenCalled();
    await waitFor(() => {
      expect(writes).toHaveBeenCalledTimes(1);
    }, { timeout: WRITE_DELAY_MS + 2000 });
    expect((writes.mock.calls[0]?.[1] ?? []).map((e) => e.id)).toEqual(["r2", "r3"]);
    await waitFor(async () => {
      expect(Object.keys(await stored(memory)).sort()).toEqual(["r1", "r2", "r3"]);
    });
    const content = await stored(memory);
    expect(content.r2).toMatchObject({ version: 2, x: 99 });
    expect(content.r1).toMatchObject({ version: 1 });
  });

  it("writes what is pending when it closes", async () => {
    const { user, editor, memory, library } = await open([]);
    await user.click(within(blockElement(editor)).getByRole("button", { name: t("whiteboard.edit") }));
    await screen.findByTestId("stand-in");
    library.change([el("a"), text("t", "Typed")]);
    await user.click(screen.getByRole("button", { name: t("whiteboard.close") }));
    await waitFor(async () => {
      expect(Object.keys(await stored(memory)).sort()).toEqual(["a", "t"]);
    });
    // The text of the drawing is the block's text in the index: search finds it.
    expect(storeOf(editor).read(BOARD)).toHaveLength(2);
  });

  it("takes in what another device draws, by version, without recording it in the editor's history", async () => {
    const { user, editor, memory, library } = await open([el("r1")]);
    await user.click(within(blockElement(editor)).getByRole("button", { name: t("whiteboard.edit") }));
    await screen.findByTestId("stand-in");
    const { page } = await idsOf(memory);
    act(() => {
      memory.changePage(
        page("Notes"),
        (doc) => {
          writeWhiteboardElements(doc, BOARD, [el("r1", 2, { x: 50 }), el("r9")]);
        },
        "Core",
      );
    });
    await waitFor(() => {
      expect(library.updates).toHaveLength(1);
    });
    expect((library.updates[0]?.elements as WhiteboardValue[]).map((e) => [e.id, e.version])).toEqual([
      ["r1", 2],
      ["r9", 1],
    ]);
    expect(library.updates[0]?.captureUpdate).toBe("never");
    // What came in is not written back.
    library.change(library.state.scene);
    await new Promise((resolve) => setTimeout(resolve, WRITE_DELAY_MS + 200));
    expect(Object.keys(await stored(memory)).sort()).toEqual(["r1", "r9"]);
  });

  it("is view only in a read-only editor: no height buttons, nothing written", async () => {
    const { user, editor, memory, library } = await open([el("r1")]);
    act(() => {
      editor.setEditable(false);
    });
    const block = blockElement(editor);
    expect(within(block).queryByRole("button", { name: t("whiteboard.taller") })).not.toBeInTheDocument();
    await user.click(within(block).getByRole("button", { name: t("whiteboard.open") }));
    expect(await screen.findByTestId("stand-in")).toHaveAttribute("data-view-mode", "true");
    expect(screen.getByText(t("whiteboard.readOnly"))).toBeInTheDocument();
    library.change([el("r1", 2), el("r2")]);
    await new Promise((resolve) => setTimeout(resolve, WRITE_DELAY_MS + 200));
    expect(Object.keys(await stored(memory))).toEqual(["r1"]);
  });

  it("goes with the page: the pending changes are written and the editor is gone", async () => {
    const { user, editor, memory, library, unmount } = await open([el("r1")]);
    await user.click(within(blockElement(editor)).getByRole("button", { name: t("whiteboard.edit") }));
    await screen.findByTestId("stand-in");
    const store = storeOf(editor);
    library.change([el("r1", 2)]);
    unmount();
    expect(library.state.mounted).toBe(0);
    expect(store.read(BOARD).find((e) => e.id === "r1")?.version).toBe(2);
    expect(await stored(memory)).toBeDefined();
  });

  it("changes the height of the picture with the buttons, within its limits", async () => {
    const { user, editor } = await open([], 200);
    const block = blockElement(editor);
    await user.click(within(block).getByRole("button", { name: t("whiteboard.taller") }));
    expect(editor.state.doc.nodeAt(whiteboardNodes(editor)[0]?.pos ?? 0)?.attrs.height).toBe(280);
    for (let i = 0; i < 3; i++) {
      await user.click(within(blockElement(editor)).getByRole("button", { name: t("whiteboard.shorter") }));
    }
    expect(editor.state.doc.nodeAt(whiteboardNodes(editor)[0]?.pos ?? 0)?.attrs.height).toBe(160);
    expect(within(blockElement(editor)).getByRole("button", { name: t("whiteboard.shorter") })).toBeDisabled();
  });
});

describe("the drawing follows its block", () => {
  it("is copied by a duplicate, under the new block id", async () => {
    const { editor } = await open([el("r1"), text("t1", "Hi"), el("gone", 3, { isDeleted: true })]);
    const store = storeOf(editor);
    const [first] = whiteboardNodes(editor);
    act(() => {
      const tr = duplicateBlock(editor.state, first?.pos ?? 0);
      if (tr !== null) {
        editor.view.dispatch(tr);
      }
    });
    await waitFor(() => {
      expect(whiteboardNodes(editor)).toHaveLength(2);
    });
    const [original, copy] = whiteboardNodes(editor);
    expect(copy?.id).not.toBe(original?.id);
    await waitFor(() => {
      expect(store.read(copy?.id ?? "")).toHaveLength(3);
    });
    expect(store.read(copy?.id ?? "").map((e) => [e.id, e.isDeleted])).toEqual([
      ["gone", true],
      ["r1", false],
      ["t1", false],
    ]);
    expect(store.read(original?.id ?? "")).toHaveLength(3);
    expect(whiteboardCopyState().pending).toBe(0);
  });

  it("is copied by a paste on the page, and a paste of a block from another page brings its drawing from the stash", async () => {
    const { editor } = await open([el("r1")]);
    const store = storeOf(editor);
    const node = editor.state.doc.nodeAt(whiteboardNodes(editor)[0]?.pos ?? 0);
    if (node === null) {
      throw new Error("no node");
    }
    // The slice a paste hands over gets a new id and a queued copy.
    const pasted = withNewWhiteboardIds(new Slice(editor.state.schema.nodes.doc!.create(null, [node]).content, 0, 0));
    const pastedId = String(pasted.content.firstChild?.attrs.id);
    expect(pastedId).not.toBe(BOARD);
    expect(whiteboardCopyState().pending).toBe(1);
    act(() => {
      editor.view.dispatch(editor.state.tr.insert(editor.state.doc.content.size, pasted.content));
    });
    await waitFor(() => {
      expect(store.read(pastedId)).toHaveLength(1);
    });
    expect(whiteboardCopyState().pending).toBe(0);

    // The source page is not open: the stash has what was copied.
    const FAR = "f0f0f0f0-f0f0-4f0f-8f0f-f0f0f0f0f0f0";
    const farNode = editor.state.schema.nodes.whiteboard!.create({ id: FAR, height: 300 });
    const farSlice = withNewWhiteboardIds(new Slice(editor.state.schema.nodes.doc!.create(null, [farNode]).content, 0, 0));
    const farId = String(farSlice.content.firstChild?.attrs.id);
    act(() => {
      editor.view.dispatch(editor.state.tr.insert(editor.state.doc.content.size, farSlice.content));
    });
    await waitFor(() => {
      expect(whiteboardCopyState().pending).toBe(0);
    });
    expect(store.read(farId)).toEqual([]); // nothing in the document and nothing stashed: an empty drawing, no error
  });

  it("stays in the document when its block is deleted, and is back with the block when the deletion is undone", async () => {
    const { editor, memory } = await open([el("r1")]);
    const store = storeOf(editor);
    const [first] = whiteboardNodes(editor);
    act(() => {
      const node = editor.state.doc.nodeAt(first?.pos ?? 0);
      editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, first?.pos ?? 0)).deleteSelection());
      expect(node?.type.name).toBe("whiteboard");
    });
    await waitFor(() => {
      expect(whiteboardNodes(editor)).toHaveLength(0);
    });
    // The portable form leaves it out; the document still holds it.
    expect(await stored(memory)).toEqual({});
    expect(store.read(BOARD)).toHaveLength(1);
    act(() => {
      undo(editor.state, editor.view.dispatch);
    });
    await waitFor(() => {
      expect(whiteboardNodes(editor)).toHaveLength(1);
    });
    expect(whiteboardNodes(editor)[0]?.id).toBe(BOARD);
    await waitFor(async () => {
      expect(Object.keys(await stored(memory))).toEqual(["r1"]);
    });
    await waitFor(() => {
      expect(blockElement(editor).querySelector(".whiteboard-svg > svg")).not.toBeNull();
    });
  });

  it("is not rewound by page undo", async () => {
    const { editor } = await open([el("r1")]);
    const store = storeOf(editor);
    act(() => {
      editor.commands.focus("end");
      editor.commands.insertContent("typed");
    });
    act(() => {
      store.write(BOARD, [el("r2")]);
    });
    expect(editor.getText()).toContain("typed");
    act(() => {
      undo(editor.state, editor.view.dispatch);
    });
    await waitFor(() => {
      expect(editor.getText()).not.toContain("typed");
    });
    expect(store.read(BOARD).map((e) => e.id)).toEqual(["r1", "r2"]);
    act(() => {
      redo(editor.state, editor.view.dispatch);
    });
    expect(store.read(BOARD)).toHaveLength(2);
  });
});
