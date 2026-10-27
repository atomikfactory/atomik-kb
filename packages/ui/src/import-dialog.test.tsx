// The Import dialog of the page menu and the space menu (ADR-0036): the destination named, the three sources,
// the progress view (indeterminate, then pages of pages), the report (counts, warnings grouped by kind and
// collapsed past ten, "Open"), a closed source dialog that says nothing, and a failure.

import { CoreError } from "@atomik-kb/core-client";
import type { ImportRunOptions, ImportReport, ImportWarning, SeedSpace, Transport } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { groupWarnings } from "./components/ImportDialog";
import { t } from "./i18n";
import { paragraphs } from "./test/editor";
import { importReport, renderInterop } from "./test/interop";
import { idsOf, openPage } from "./test/page";
import type { User } from "./test/render";

afterEach(() => {
  vi.restoreAllMocks();
});

const SPACES: SeedSpace[] = [
  {
    name: "Personal",
    pages: [
      { title: "Notes", content: paragraphs("Hello") },
      { title: "Imported", content: paragraphs("The first page the import made") },
    ],
  },
];

type ImportRun = NonNullable<Transport["importRun"]>;

function importMock(report: ImportReport = importReport()) {
  return vi.fn<ImportRun>(() => Promise.resolve(report));
}

async function openSpaceImport(user: User): Promise<HTMLElement> {
  await user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
  await user.click(await screen.findByRole("menuitem", { name: t("interop.menu.importSpace") }));
  return screen.findByRole("dialog", { name: t("import.title") });
}

async function openPageImport(user: User): Promise<HTMLElement> {
  await user.click(screen.getByRole("button", { name: t("page.actions") }));
  await user.click(await screen.findByRole("menuitem", { name: t("interop.menu.importPage") }));
  return screen.findByRole("dialog", { name: t("import.title") });
}

function warnings(code: ImportWarning["code"], count: number, detail?: string): ImportWarning[] {
  return Array.from({ length: count }, (_, index) => ({
    path: `docs/${code}-${String(index + 1)}.md`,
    code,
    ...(detail === undefined ? {} : { detail }),
  }));
}

describe("the choice of a source", () => {
  it("names the space as the destination and offers files, a folder and a .zip", async () => {
    const { user } = await renderInterop({ spaces: SPACES }, { methods: { importRun: importMock() } });
    const dialog = await openSpaceImport(user);
    expect(within(dialog).getByText(t("import.intro.space", { name: "Personal" }))).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: t("import.files") })).toHaveAccessibleDescription(t("import.files.hint"));
    expect(within(dialog).getByRole("button", { name: t("import.folder") })).toHaveAccessibleDescription(t("import.folder.hint"));
    expect(within(dialog).getByRole("button", { name: t("import.zip") })).toHaveAccessibleDescription(t("import.zip.hint"));
    await user.click(within(dialog).getByRole("button", { name: t("dialog.cancel") }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("names the page as the destination and sends it as the parent", async () => {
    const importRun = importMock();
    const { user, memory } = await renderInterop({ spaces: SPACES }, { methods: { importRun } });
    const ids = await idsOf(memory);
    await openPage(user, "Notes");
    const dialog = await openPageImport(user);
    expect(within(dialog).getByText(t("import.intro.page", { name: "Notes" }))).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("import.folder") }));
    await waitFor(() => {
      expect(importRun).toHaveBeenCalledWith({
        spaceId: ids.space("Personal"),
        parentId: ids.page("Notes"),
        source: "folder",
      } satisfies ImportRunOptions);
    });
  });

  it("sends the root of the space, with no parent, for the space menu", async () => {
    const importRun = importMock();
    const { user, memory } = await renderInterop({ spaces: SPACES }, { methods: { importRun } });
    const ids = await idsOf(memory);
    const dialog = await openSpaceImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("import.zip") }));
    await waitFor(() => {
      expect(importRun).toHaveBeenCalledTimes(1);
    });
    const options = importRun.mock.calls[0]?.[0];
    expect(options).toMatchObject({ spaceId: ids.space("Personal"), source: "zip" });
    expect(options?.parentId).toBeUndefined();
  });

  it("is not offered when the shell cannot import", async () => {
    const { user } = await renderInterop({ spaces: SPACES });
    await user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
    await screen.findByRole("menu", { name: t("spaces.optionsLabel") });
    expect(screen.queryByRole("menuitem", { name: t("interop.menu.importSpace") })).not.toBeInTheDocument();
  });

  it("says nothing when the source dialog is closed, and offers the choice again", async () => {
    const importRun = vi.fn<ImportRun>(() =>
      Promise.reject(new CoreError({ code: "InvalidInput", message: "no file was chosen", details: { field: "path" } })),
    );
    const { user } = await renderInterop({ spaces: SPACES }, { methods: { importRun } });
    const dialog = await openSpaceImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("import.files") }));
    await waitFor(() => {
      expect(importRun).toHaveBeenCalledTimes(1);
      expect(within(dialog).getByRole("button", { name: t("import.files") })).toBeInTheDocument();
    });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows why an import failed and goes back to the choice", async () => {
    const importRun = vi.fn<ImportRun>(() => Promise.reject(new CoreError({ code: "Storage", message: "disk full" })));
    const { user } = await renderInterop({ spaces: SPACES }, { methods: { importRun } });
    const dialog = await openSpaceImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("import.zip") }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("import.error", { message: "disk full" }));
    expect(within(dialog).getByRole("button", { name: t("import.zip") })).toBeInTheDocument();
  });
});

describe("the progress", () => {
  it("is indeterminate while files are read and uploaded, then pages written of pages to write", async () => {
    let finish: (report: ImportReport) => void = () => undefined;
    const importRun = vi.fn<ImportRun>(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const { user, emit } = await renderInterop({ spaces: SPACES }, { methods: { importRun } });
    const dialog = await openSpaceImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("import.zip") }));

    expect(await screen.findByRole("dialog", { name: t("import.running") })).toBe(dialog);
    expect(within(dialog).getByRole("status")).toHaveTextContent(t("import.reading"));
    const bar = within(dialog).getByLabelText(t("import.progressLabel"));
    expect(bar).not.toHaveAttribute("value");
    // The source choice is gone and the dialog cannot be dismissed while it works.
    expect(within(dialog).queryByRole("button", { name: t("import.zip") })).not.toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.getByRole("dialog")).toBe(dialog);

    act(() => {
      emit({ type: "ImportProgress", data: { importId: 1, done: 3, total: 12 } });
    });
    expect(within(dialog).getByRole("status")).toHaveTextContent(t("import.writing", { done: 3, total: 12 }));
    const determinate = within(dialog).getByLabelText(t("import.progressLabel"));
    expect(determinate).toHaveAttribute("value", "3");
    expect(determinate).toHaveAttribute("max", "12");

    await act(async () => {
      finish(importReport({ pages: 12 }));
      await Promise.resolve();
    });
    expect(await screen.findByRole("dialog", { name: t("import.report.title") })).toBe(dialog);
  });
});

describe("the report", () => {
  it("counts what was created, with nothing left out", async () => {
    const importRun = importMock(importReport({ pages: 7, attachments: 3, propertiesCreated: 2 }));
    const { user } = await renderInterop({ spaces: SPACES }, { methods: { importRun } });
    const dialog = await openSpaceImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("import.files") }));

    const report = await screen.findByRole("dialog", { name: t("import.report.title") });
    const pairs = Array.from(report.querySelectorAll("dt")).map((term) => [term.textContent, term.nextElementSibling?.textContent]);
    expect(pairs).toEqual([
      [t("import.report.pages"), "7"],
      [t("import.report.attachments"), "3"],
      [t("import.report.properties"), "2"],
    ]);
    expect(within(report).getByText(t("import.report.noWarnings"))).toBeInTheDocument();
    expect(within(report).queryByRole("button", { name: t("import.report.open") })).not.toBeInTheDocument();
    // Focus moved to the heading of the step, inside the dialog.
    expect(report.contains(document.activeElement)).toBe(true);
  });

  it("groups the warnings by kind with their files, and collapses a group of more than ten", async () => {
    const list = [
      ...warnings("unresolvedLink", 2, "missing.md"),
      ...warnings("remoteImage", 11),
      ...warnings("unsupportedFile", 10),
    ];
    const importRun = importMock(importReport({ warnings: list, moreWarnings: 40 }));
    const { user } = await renderInterop({ spaces: SPACES }, { methods: { importRun } });
    const dialog = await openSpaceImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("import.zip") }));
    const report = await screen.findByRole("dialog", { name: t("import.report.title") });

    const groups = Array.from(report.querySelectorAll("details"));
    expect(groups.map((group) => group.querySelector("summary")?.textContent)).toEqual([
      t("import.report.group", { label: t("import.warning.unresolvedLink"), count: 2 }),
      t("import.report.group", { label: t("import.warning.remoteImage"), count: 11 }),
      t("import.report.group", { label: t("import.warning.unsupportedFile"), count: 10 }),
    ]);
    // Up to ten files start open; more start collapsed.
    expect(groups.map((group) => group.open)).toEqual([true, false, true]);
    const first = groups[0];
    expect(Array.from(first?.querySelectorAll("li") ?? []).map((item) => item.textContent)).toEqual([
      "docs/unresolvedLink-1.mdmissing.md",
      "docs/unresolvedLink-2.mdmissing.md",
    ]);
    expect(groups[1]?.querySelectorAll("li")).toHaveLength(11);
    expect(within(report).getByText(t("import.report.more", { count: 40 }))).toBeInTheDocument();
    expect(within(report).queryByText(t("import.report.noWarnings"))).not.toBeInTheDocument();
  });

  it("opens the first page created and closes", async () => {
    const importRun = vi.fn<ImportRun>(() => Promise.resolve(importReport()));
    const { user, memory, store } = await renderInterop({ spaces: SPACES }, { methods: { importRun } });
    const ids = await idsOf(memory);
    importRun.mockResolvedValue(importReport({ rootPageIds: [ids.page("Imported"), ids.page("Notes")] }));
    const dialog = await openSpaceImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("import.zip") }));
    const report = await screen.findByRole("dialog", { name: t("import.report.title") });
    await user.click(within(report).getByRole("button", { name: t("import.report.open") }));

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(store.getState().selectedPageId).toBe(ids.page("Imported"));
    expect(await screen.findByRole("heading", { level: 1, name: "Imported" })).toBeInTheDocument();
  });

  it("closes with the Close button and with Escape", async () => {
    const { user } = await renderInterop({ spaces: SPACES }, { methods: { importRun: importMock() } });
    const dialog = await openSpaceImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("import.files") }));
    const report = await screen.findByRole("dialog", { name: t("import.report.title") });
    await user.click(within(report).getByRole("button", { name: t("dialog.close") }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    const again = await openSpaceImport(user);
    await user.click(within(again).getByRole("button", { name: t("import.files") }));
    await screen.findByRole("dialog", { name: t("import.report.title") });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});

describe("groupWarnings", () => {
  it("groups by kind in the order each kind first appears", () => {
    const list: ImportWarning[] = [
      { path: "a.md", code: "unresolvedLink" },
      { path: "b.md", code: "notUtf8" },
      { path: "c.md", code: "unresolvedLink", detail: "x" },
    ];
    expect(groupWarnings(list).map((group) => [group.code, group.entries.length])).toEqual([
      ["unresolvedLink", 2],
      ["notUtf8", 1],
    ]);
    expect(groupWarnings([])).toEqual([]);
  });
});
