// Helpers for the tests of a database block's toolbar (ADR-0054): its bar (Filter, Sort, Properties, view settings) and the
// popover each button opens. The popovers are floating panels in the page's body, so a control is found on the screen,
// after the test opened the popover that holds it.

import { screen, within } from "@testing-library/react";

import { t } from "../i18n";
import type { User } from "./render";

/** The popovers of the bar. */
export type ViewPanel = "filter" | "sort" | "properties" | "settings";

/** The accessible name of each popover. */
const DIALOGS: Readonly<Record<ViewPanel, () => string>> = {
  filter: () => t("pageList.filter.label"),
  sort: () => t("pageList.sort"),
  properties: () => t("pageList.properties.label"),
  settings: () => t("pageList.viewSettings"),
};

/** The bar of a block. */
export function viewToolbar(block: HTMLElement): HTMLElement {
  return within(block).getByRole("toolbar", { name: t("pageList.toolbar") });
}

/** The button that opens a popover (Filter, Sort and Properties may carry a count: they are matched by their first word). */
export function panelButton(block: HTMLElement, panel: ViewPanel): HTMLElement {
  const bar = viewToolbar(block);
  switch (panel) {
    case "filter":
      return within(bar).getByRole("button", { name: new RegExp(`^${t("pageList.filter")}`, "u") });
    case "sort":
      return within(bar).getByRole("button", { name: new RegExp(`^${t("pageList.sort")}`, "u") });
    case "properties":
      return within(bar).getByRole("button", { name: new RegExp(`^${t("pageList.properties")}`, "u") });
    case "settings":
      return within(bar).getByRole("button", { name: t("pageList.viewSettings") });
  }
}

/** Opens a popover of the bar (it stays as it is when it is open already) and returns it. Opening one closes the other. */
export async function openPanel(user: User, block: HTMLElement, panel: ViewPanel): Promise<HTMLElement> {
  const name = DIALOGS[panel]();
  const open = screen.queryByRole("dialog", { name });
  if (open !== null) {
    return open;
  }
  await user.click(panelButton(block, panel));
  return screen.findByRole("dialog", { name });
}

/** The popover that holds each control a test asks for by its name. */
function panelOf(label: string): ViewPanel {
  return label === t("pageList.sort.label") || label === t("pageList.sort.dir") ? "sort" : "settings";
}

/**
 * The select named `label`, wherever the bar keeps it: "Sort by" and "Sort order" in the Sort popover, the layout, the
 * source, "Open pages in" and the layout's own selects (the date, the card size, the group by) in the view settings
 * popover. Opens the popover when it is not open.
 */
export async function viewSelect(user: User, block: HTMLElement, label: string): Promise<HTMLSelectElement> {
  const panel = await openPanel(user, block, panelOf(label));
  return within(panel).getByRole<HTMLSelectElement>("combobox", { name: label });
}

/** The select `label` of the popover that is open (the test opened it: `viewSelect` or `openPanel`). */
export function openSelect(label: string): HTMLSelectElement {
  return screen.getByRole<HTMLSelectElement>("combobox", { name: label });
}
