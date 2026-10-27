// Helpers for the tests of templates in the UI (ADR-0056): the whole layout over an in-memory core that records the requests, and
// the ids of its pages by title.

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { PageId, PageInfo, Request, Response, SeedSpace } from "@atomik-kb/core-client";
import { render } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { onTestFinished } from "vitest";

import { MainLayout } from "../screens/MainLayout";
import { createAppStore } from "../store";
import { StoreContext } from "../store-context";
import { interopTransport } from "./interop";
import { NOW, findMainNavigation } from "./render";

/** A space with a few pages and templates: Journal and Projects (with Roadmap), a top-level template and one under Projects. */
export const TEMPLATE_SPACE: SeedSpace = {
  name: "Personal",
  icon: "🏠",
  properties: [
    { name: "Due", kind: "date" },
    { name: "Owner", kind: "person" },
    { name: "Priority", kind: "number" },
  ],
  pages: [
    { title: "Journal" },
    {
      title: "Projects",
      children: [{ title: "Roadmap" }, { title: "Project brief", template: true, children: [{ title: "Brief notes" }] }],
    },
    {
      title: "Meeting notes",
      icon: "📝",
      template: true,
      properties: { Due: { date: "2026-10-15" } },
      children: [{ title: "Agenda" }],
    },
    { title: "Reading list" },
  ],
};

export const OTHER_SPACE: SeedSpace = { name: "Work", pages: [{ title: "Meetings" }] };

export interface TemplateAppOptions {
  /** Answers a request itself (`undefined` leaves it to the memory core). */
  intercept?: (request: Request, memory: MemoryTransport) => Response | Promise<Response> | undefined;
}

/** Renders the whole layout over a memory core with `spaces` (the template space by default) and waits for the sidebar. */
export async function renderTemplateApp(spaces: readonly SeedSpace[] = [TEMPLATE_SPACE], options: TemplateAppOptions = {}) {
  const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", spaces: [...spaces] });
  const direct = new CoreClient(memory);
  const wrapped = interopTransport(memory, { intercept: (request) => options.intercept?.(request, memory) });
  const store = createAppStore(new CoreClient(wrapped.transport), { corePollMs: 5 });
  const stopStore = store.getState().start();
  onTestFinished(() => {
    stopStore();
    wrapped.detach();
  });
  const user = userEvent.setup();
  const view = render(
    <StoreContext value={store}>
      <MainLayout />
    </StoreContext>,
  );
  await findMainNavigation();
  return { memory, store, user, client: direct, requests: wrapped.requests, ...view };
}

/** The live pages of every space by title (templates and the pages inside them included), read from the core. */
export async function pagesByTitle(memory: MemoryTransport): Promise<ReadonlyMap<string, PageInfo>> {
  const client = new CoreClient(memory);
  const pages = new Map<string, PageInfo>();
  for (const space of await client.spaceList()) {
    const visit = (nodes: Awaited<ReturnType<CoreClient["treeGet"]>>["roots"]): void => {
      for (const node of nodes) {
        pages.set(node.page.title, node.page);
        visit(node.children);
      }
    };
    visit((await client.treeGet(space.id)).roots);
  }
  return pages;
}

/** The id of a live page by title. */
export async function pageIdOf(memory: MemoryTransport, title: string): Promise<PageId> {
  const page = (await pagesByTitle(memory)).get(title);
  if (page === undefined) {
    throw new Error(`no page titled ${title}`);
  }
  return page.id;
}
