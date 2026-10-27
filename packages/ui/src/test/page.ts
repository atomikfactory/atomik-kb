// Helpers for tests of the page view and the editor inside the whole app.

import { CoreClient } from "@atomik-kb/core-client";
import type { JsonValue, MemoryTransport, PageId, SeedSpace, SpaceId } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { screen, waitFor } from "@testing-library/react";
import { LoroList, LoroMap, LoroText } from "loro-crdt";
import type { LoroDoc } from "loro-crdt";
import { expect } from "vitest";

import { loroSyncPluginKey } from "../editor/loro-binding";
import { t } from "../i18n";
import { findMainNavigation, renderApp } from "./render";
import type { User } from "./render";
import { row } from "./sidebar";

/** Renders the app over a workspace with `spaces` and waits for the sidebar. */
export async function renderPages(spaces: readonly SeedSpace[], options: Parameters<typeof renderApp>[0] = {}) {
  const app = renderApp({ workspaceName: "Research", spaces, ...options });
  await findMainNavigation();
  return app;
}

/** Page and space ids by page title, read from the core. */
export async function idsOf(memory: MemoryTransport): Promise<{ page: (title: string) => PageId; space: (name: string) => SpaceId }> {
  const client = new CoreClient(memory);
  const pages = new Map<string, PageId>();
  const spaces = new Map<string, SpaceId>();
  for (const space of await client.spaceList()) {
    spaces.set(space.name, space.id);
    const tree = await client.treeGet(space.id);
    const visit = (nodes: typeof tree.roots): void => {
      for (const node of nodes) {
        pages.set(node.page.title, node.page.id);
        visit(node.children);
      }
    };
    visit(tree.roots);
  }
  return {
    page: (title) => pages.get(title) ?? "",
    space: (name) => spaces.get(name) ?? "",
  };
}

/** The Tiptap editor behind the page editor's text box. */
export function editorOf(element: HTMLElement): Editor {
  const editor = (element as HTMLElement & { editor?: Editor }).editor;
  if (editor === undefined) {
    throw new Error("not an editor");
  }
  return editor;
}

/** Waits for the page editor of the open page and for the binding to show the document. */
export async function findEditor(): Promise<Editor> {
  const box = await screen.findByRole("textbox", { name: t("editor.label") });
  const editor = editorOf(box);
  await waitFor(() => {
    expect(loroSyncPluginKey.getState(editor.state)?.docSubscription).toBeTruthy();
  });
  return editor;
}

/** Opens a page of the current space from the sidebar and returns its editor. */
export async function openPage(user: User, title: string): Promise<Editor> {
  await user.click(row(title));
  return findEditor();
}

/** The page's content as the core holds it (portable JSON). */
export async function contentOf(memory: MemoryTransport, pageId: PageId): Promise<JsonValue> {
  return (await new CoreClient(memory).pageContentGet(pageId)).content;
}

/** Appends a paragraph `text` + reference to `pageId` to a page document, as another writer would. */
export function appendReference(doc: LoroDoc, text: string, pageId: PageId, blockId: string): void {
  const children = doc.getMap("doc").get("children") as LoroList;
  const paragraph = children.insertContainer(children.length, new LoroMap());
  paragraph.set("nodeName", "paragraph");
  paragraph.setContainer("attributes", new LoroMap()).set("id", blockId);
  const inline = paragraph.setContainer("children", new LoroList());
  inline.insertContainer(0, new LoroText()).insert(0, text);
  const ref = inline.insertContainer(1, new LoroMap());
  ref.set("nodeName", "pageRef");
  const attributes = ref.setContainer("attributes", new LoroMap());
  attributes.set("id", `${blockId.slice(0, -4)}ffff`);
  attributes.set("pageId", pageId);
  ref.setContainer("children", new LoroList());
}

/** A clipboard event carrying HTML and text, as a paste would deliver it. */
export function pasteEvent(html: string, text: string): Event {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  const data = {
    types: ["text/html", "text/plain"],
    getData: (type: string) => (type === "text/html" ? html : type === "text/plain" ? text : ""),
  };
  Object.defineProperty(event, "clipboardData", { value: data });
  return event;
}
