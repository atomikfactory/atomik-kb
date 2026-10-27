// Helpers for the tests of page properties (ADR-0026): a seeded space that has every kind, the
// panel of the open page, and reading what the core holds back.

import { CoreClient } from "@atomik-kb/core-client";
import type { MemoryTransport, PropertyDef, PropertyValue, SeedSpace } from "@atomik-kb/core-client";
import { screen, within } from "@testing-library/react";

import { t } from "../i18n";
import { idsOf, renderPages } from "./page";
import type { User } from "./render";
import { row } from "./sidebar";

/**
 * "Personal": two tags, a property of every kind (Status has the three default options, Stage and
 * Areas their own), and the pages "Notes" (a value of each kind), "Plain" (none) and "Other" (a tag).
 */
export const PROPERTY_SPACE: SeedSpace = {
  name: "Personal",
  tagOptions: [
    { name: "idea", color: "green" },
    { name: "project", color: "blue" },
  ],
  properties: [
    { name: "Status", kind: "status" },
    { name: "Due", kind: "date" },
    { name: "Done", kind: "checkbox" },
    { name: "Link", kind: "url" },
    { name: "Owner email", kind: "email" },
    { name: "Priority", kind: "number" },
    { name: "Summary", kind: "text" },
    { name: "Phone", kind: "phone" },
    {
      name: "Stage",
      kind: "select",
      options: [
        { name: "Draft", color: "gray" },
        { name: "Review", color: "orange" },
      ],
    },
    {
      name: "Areas",
      kind: "multi_select",
      options: [
        { name: "Home", color: "pink" },
        { name: "Work", color: "purple" },
      ],
    },
  ],
  pages: [
    {
      title: "Notes",
      tags: ["idea"],
      properties: {
        Status: "In progress",
        Due: { date: "2026-10-15" },
        Done: true,
        Link: "https://example.com",
        "Owner email": "ada@example.com",
        Priority: 3,
        Summary: "Draft",
        Phone: "+1 555 0100",
        Stage: "Draft",
        Areas: ["Home"],
      },
    },
    { title: "Plain" },
    { title: "Other", tags: ["project"] },
  ],
};

/** Renders the app over `spaces` (the property space by default) and waits for the sidebar. */
export function renderProperties(
  spaces: readonly SeedSpace[] = [PROPERTY_SPACE],
  options: Parameters<typeof renderPages>[1] = {},
) {
  return renderPages(spaces, options);
}

/** Opens a page from the sidebar and returns its properties panel. */
export async function openProperties(user: User, title: string): Promise<HTMLElement> {
  await user.click(row(title));
  return screen.findByRole("region", { name: t("properties.title") });
}

/** The row of a property in the panel. */
export function propertyRow(panel: HTMLElement, name: string): HTMLElement {
  return within(panel).getByRole("listitem", { name });
}

/** The names of the rows of the panel, in order (the items of a checklist and the chips of files are no rows). */
export function rowNames(panel: HTMLElement): string[] {
  return within(panel)
    .getAllByRole("listitem")
    .filter((item) => item.classList.contains("property-row"))
    .map((item) => item.getAttribute("aria-label") ?? "");
}

/** The schema of a space as the core has it. */
export async function schemaOf(memory: MemoryTransport, spaceName = "Personal"): Promise<PropertyDef[]> {
  const { space } = await idsOf(memory);
  return new CoreClient(memory).propertySchemaGet(space(spaceName));
}

/** The value a page holds for the property with this name (`undefined` when it has none). */
export async function valueOf(
  memory: MemoryTransport,
  pageTitle: string,
  propertyName: string,
  spaceName = "Personal",
): Promise<PropertyValue | undefined> {
  const client = new CoreClient(memory);
  const { page } = await idsOf(memory);
  const def = (await schemaOf(memory, spaceName)).find((candidate) => candidate.name === propertyName);
  return (await client.pagePropertiesGet(page(pageTitle))).find(([id]) => id === def?.id)?.[1];
}

/** The id of the option with this name in a property. */
export async function optionIdOf(memory: MemoryTransport, propertyName: string, optionName: string): Promise<string> {
  const def = (await schemaOf(memory)).find((candidate) => candidate.name === propertyName);
  return def?.options.find((option) => option.name === optionName)?.id ?? "";
}
