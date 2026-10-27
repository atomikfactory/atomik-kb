// The built-in starters (ADR-0056, `docs/architecture/templates.md` section 8) through the Templates panel and the picker of the app,
// with the real starters and the real import session of the in-memory core (`templates.test.tsx` replaces them by two that make a
// page through the client): the five are listed in the language of the app, "Use" makes a page with the day resolved and opens it,
// and "Add to my templates" makes a template page of the space that keeps its title tokens and is listed at once.

import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t } from "./i18n";
import { STARTER_TEMPLATES } from "./starter-templates";
import { outline, sidebarButton } from "./test/sidebar";
import { renderTemplateApp } from "./test/templates";
import type { User } from "./test/render";

async function openPanel(user: User): Promise<HTMLElement> {
  await user.click(sidebarButton(t("template.panel.open")));
  return screen.findByRole("dialog", { name: t("template.panel.title") });
}

function lineOf(container: HTMLElement, title: string): HTMLElement {
  const line = within(container).getByText(title).closest("li");
  if (line === null) {
    throw new Error(`no line for ${title}`);
  }
  return line;
}

const SPACE = { name: "Personal", pages: [{ title: "Journal" }] };

describe("the built-in starters in the Templates panel", () => {
  it("lists the five starters with their names and descriptions in the language of the app", async () => {
    const { user } = await renderTemplateApp([SPACE]);
    const dialog = await openPanel(user);
    const section = within(dialog).getByRole("region", { name: t("template.panel.starters") });
    expect(STARTER_TEMPLATES).toHaveLength(5);
    for (const starter of STARTER_TEMPLATES) {
      const line = lineOf(section, starter.title("en"));
      expect(within(line).getByText(starter.description("en"))).toBeInTheDocument();
    }
  });

  it("uses a starter: a page with no unresolved token is made in the space and opened", async () => {
    const { user, store, client } = await renderTemplateApp([SPACE]);
    const dialog = await openPanel(user);
    const first = STARTER_TEMPLATES[0];
    expect(first).toBeDefined();
    await user.click(within(lineOf(dialog, first?.title("en") ?? "")).getByRole("button", { name: t("template.use") }));
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: t("template.panel.title") })).not.toBeInTheDocument();
    });
    await waitFor(() => {
      expect(store.getState().selectedPageId).not.toBeNull();
    });
    const made = await client.pageGet(store.getState().selectedPageId ?? "");
    expect(made.template).toBeUndefined();
    expect(made.title).not.toContain("{{");
    expect(made.title).not.toBe("");
    // It is in the sidebar, opened.
    await waitFor(() => {
      expect(outline()).toContain(made.title);
    });
    expect(await screen.findByRole("heading", { level: 1, name: made.title })).toBeInTheDocument();
  });

  it("adds a starter to the templates of the space: a template page, listed at once, nothing opens, not in the sidebar", async () => {
    const { user, store, client } = await renderTemplateApp([SPACE]);
    const dialog = await openPanel(user);
    const starter = STARTER_TEMPLATES[1];
    expect(starter).toBeDefined();
    await user.click(within(lineOf(dialog, starter?.title("en") ?? "")).getByRole("button", { name: t("template.addToMine") }));
    const top = within(dialog).getByRole("region", { name: t("template.panel.groupTop") });
    await within(top).findByRole("listitem");
    const [space] = await client.spaceList();
    const templates = (await client.treeGet(space?.id ?? "")).roots.filter((node) => node.page.template === true);
    expect(templates).toHaveLength(1);
    expect(store.getState().selectedPageId).toBeNull();
    expect(outline()).toEqual(["Journal"]);
    expect(screen.getByRole("dialog", { name: t("template.panel.title") })).toBeInTheDocument();
  });
});
