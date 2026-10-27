// Shared fixtures and queries for the sidebar tests.

import type { SeedSpace } from "@atomik-kb/core-client";
import { screen, within } from "@testing-library/react";

import { t } from "../i18n";
import { findMainNavigation, renderApp } from "./render";

/** A space with a nested tree: Projects (Roadmap (Q1), Ideas), Journal, Reading list. */
export const PERSONAL: SeedSpace = {
  name: "Personal",
  icon: "🏠",
  pages: [
    {
      title: "Projects",
      icon: "📁",
      children: [{ title: "Roadmap", children: [{ title: "Q1" }] }, { title: "Ideas" }],
    },
    { title: "Journal" },
    { title: "Reading list" },
  ],
};

export const PERSONAL_OUTLINE = ["Projects", "  Roadmap", "    Q1", "  Ideas", "Journal", "Reading list"];

/** A second space, with one page. */
export const WORK: SeedSpace = { name: "Work", pages: [{ title: "Meetings" }] };

/** Renders the app over a workspace with `spaces` (Personal and Work by default) and waits for it. */
export async function renderSidebar(options: Parameters<typeof renderApp>[0] = {}) {
  const app = renderApp({ workspaceName: "Research", spaces: [PERSONAL, WORK], ...options });
  await findMainNavigation();
  return app;
}

/** The page tree; throws when there are no pages (the empty state has no tree). */
export function pageTree(): HTMLElement {
  return screen.getByRole("tree", { name: t("tree.label") });
}

/** One row of the page tree, found by the page title. */
export function row(title: string): HTMLElement {
  return within(pageTree()).getByRole("treeitem", { name: title });
}

/** The rows of the tree as titles indented by depth, for example `["A", "  A1", "B"]`. */
export function outline(): string[] {
  const tree = screen.queryByRole("tree", { name: t("tree.label") });
  if (tree === null) {
    return [];
  }
  return within(tree)
    .getAllByRole("treeitem")
    .map((item) => {
      const level = Number(item.getAttribute("aria-level") ?? "1");
      return `${"  ".repeat(level - 1)}${item.getAttribute("aria-label") ?? ""}`;
    });
}

/** A button of the sidebar, found by its accessible name. */
export function sidebarButton(name: string): HTMLElement {
  return within(screen.getByRole("navigation", { name: t("nav.label") })).getByRole("button", { name });
}
