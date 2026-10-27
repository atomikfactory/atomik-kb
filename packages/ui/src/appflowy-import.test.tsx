// "Import AppFlowy workspace…" of the space menu (ADR-0039): the entry (only when the shell can import), the dialog that
// says where AppFlowy makes its backup, the one choice (the backup .zip, into new spaces), the progress view it shares with the
// Import dialog, and the report that lists the new spaces with their pages and "Open" for each.

import { CoreClient, CoreError, demoImportSources } from "@atomik-kb/core-client";
import type { ImportReport, ImportRunOptions, SeedSpace, SpaceId, Transport } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { Mock } from "vitest";

import { t, tp } from "./i18n";
import { importReport, renderInterop } from "./test/interop";
import { idsOf } from "./test/page";
import type { User } from "./test/render";
import { PERSONAL, WORK, outline } from "./test/sidebar";

type ImportRun = NonNullable<Transport["importRun"]>;

const IMPORTED: SeedSpace[] = [
  PERSONAL,
  WORK,
  {
    name: "Studio",
    icon: "🎬",
    pages: [{ title: "Shots", children: [{ title: "Shot 1" }, { title: "Shot 2" }] }, { title: "Notes" }],
  },
];

async function openAppFlowyImport(user: User): Promise<HTMLElement> {
  await user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
  await user.click(await screen.findByRole("menuitem", { name: t("appflowyImport.menu") }));
  return screen.findByRole("dialog", { name: t("appflowyImport.title") });
}

async function setup(importRun: Mock<ImportRun> = vi.fn<ImportRun>(() => Promise.resolve(importReport()))) {
  const app = await renderInterop({ spaces: IMPORTED }, { methods: { importRun } });
  const ids = await idsOf(app.memory);
  return { ...app, ids, importRun };
}

describe("the entry", () => {
  it("follows Import... in the space menu", async () => {
    const { user } = await setup();
    await user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
    const menu = await screen.findByRole("menu", { name: t("spaces.optionsLabel") });
    const names = within(menu)
      .getAllByRole("menuitem")
      .map((item) => item.textContent);
    expect(names.indexOf(t("appflowyImport.menu"))).toBe(names.indexOf(t("interop.menu.importSpace")) + 1);
  });

  it("is not offered when the shell cannot import", async () => {
    const { user } = await renderInterop({ spaces: IMPORTED });
    await user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
    await screen.findByRole("menu", { name: t("spaces.optionsLabel") });
    expect(screen.queryByRole("menuitem", { name: t("appflowyImport.menu") })).not.toBeInTheDocument();
  });
});

describe("the dialog", () => {
  it("says where AppFlowy makes the backup and offers the backup .zip", async () => {
    const { user } = await setup();
    const dialog = await openAppFlowyImport(user);
    expect(within(dialog).getByText(t("appflowyImport.how"))).toHaveTextContent(
      "Settings > Manage data > Back up your workspace",
    );
    expect(within(dialog).getByText(t("appflowyImport.intro"))).toBeInTheDocument();
    expect(within(dialog).getByText(t("appflowyImport.privacy"))).toBeInTheDocument();
    expect(dialog).toHaveAccessibleDescription(t("appflowyImport.intro"));
    expect(within(dialog).getByRole("button", { name: t("appflowyImport.choose") })).toBeInTheDocument();
    // Not the sources of the Import dialog: this one has a single choice.
    expect(within(dialog).queryByRole("button", { name: t("import.files") })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: t("import.folder") })).not.toBeInTheDocument();
  });

  it("closes with Cancel and with Escape, importing nothing", async () => {
    const { user, importRun } = await setup();
    const dialog = await openAppFlowyImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("dialog.cancel") }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await openAppFlowyImport(user);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(importRun).not.toHaveBeenCalled();
  });

  it("imports the backup into new spaces, from the current space, with the destination new spaces", async () => {
    const { user, importRun, ids } = await setup();
    const dialog = await openAppFlowyImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("appflowyImport.choose") }));
    await waitFor(() => {
      expect(importRun).toHaveBeenCalledTimes(1);
    });
    expect(importRun.mock.calls[0]?.[0]).toEqual({
      spaceId: ids.space("Personal"),
      source: "zip",
      destination: "newSpaces",
    } satisfies ImportRunOptions);
  });

  it("shows the progress of the Import dialog: reading, then pages of pages", async () => {
    let finish: (report: ImportReport) => void = () => undefined;
    const importRun = vi.fn<ImportRun>(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const { user, emit } = await setup(importRun);
    const dialog = await openAppFlowyImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("appflowyImport.choose") }));
    expect(await screen.findByRole("dialog", { name: t("import.running") })).toBe(dialog);
    expect(within(dialog).getByRole("status")).toHaveTextContent(t("import.reading"));
    // It cannot be dismissed while it works.
    await user.keyboard("{Escape}");
    expect(screen.getByRole("dialog")).toBe(dialog);

    act(() => {
      emit({ type: "ImportProgress", data: { importId: 1, done: 20, total: 80 } });
    });
    expect(within(dialog).getByRole("status")).toHaveTextContent(t("import.writing", { done: 20, total: 80 }));
    await act(async () => {
      finish(importReport({ pages: 80 }));
      await Promise.resolve();
    });
    expect(await screen.findByRole("dialog", { name: t("import.report.title") })).toBe(dialog);
  });

  it("says nothing when the file dialog is closed, and offers the choice again", async () => {
    const importRun = vi.fn<ImportRun>(() =>
      Promise.reject(new CoreError({ code: "InvalidInput", message: "no file was chosen", details: { field: "path" } })),
    );
    const { user } = await setup(importRun);
    const dialog = await openAppFlowyImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("appflowyImport.choose") }));
    await waitFor(() => {
      expect(importRun).toHaveBeenCalledTimes(1);
      expect(within(dialog).getByRole("button", { name: t("appflowyImport.choose") })).toBeInTheDocument();
    });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows why an import failed, still on the explanation, and can be tried again", async () => {
    const importRun = vi.fn<ImportRun>(() =>
      Promise.reject(new CoreError({ code: "InvalidInput", message: "this is not an AppFlowy backup" })),
    );
    const { user } = await setup(importRun);
    const dialog = await openAppFlowyImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("appflowyImport.choose") }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      t("import.error", { message: "this is not an AppFlowy backup" }),
    );
    expect(within(dialog).getByText(t("appflowyImport.how"))).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: t("appflowyImport.choose") })).toBeInTheDocument();
  });
});

describe("the report", () => {
  function twoSpaces(ids: { space: (name: string) => SpaceId }): ImportReport {
    return importReport({
      pages: 8,
      attachments: 3,
      propertiesCreated: 2,
      spaceIds: [ids.space("Studio"), ids.space("Work")],
      rootPageIds: [],
    });
  }

  it("counts what was created and lists the new spaces with their pages, from the spaces list", async () => {
    const importRun = vi.fn<ImportRun>(() => Promise.resolve(importReport()));
    const { user, ids } = await setup(importRun);
    importRun.mockResolvedValue(twoSpaces(ids));
    const dialog = await openAppFlowyImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("appflowyImport.choose") }));
    const report = await screen.findByRole("dialog", { name: t("import.report.title") });

    const pairs = Array.from(report.querySelectorAll("dt")).map((term) => [term.textContent, term.nextElementSibling?.textContent]);
    expect(pairs).toEqual([
      [t("import.report.pages"), "8"],
      [t("import.report.attachments"), "3"],
      [t("import.report.properties"), "2"],
    ]);
    const list = within(report).getByRole("region", { name: t("appflowyImport.report.spaces") });
    expect(within(list).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      `🎬Studio${tp("spaces.pageCount", 4)}${t("appflowyImport.report.open")}`,
      `Work${tp("spaces.pageCount", 1)}${t("appflowyImport.report.open")}`,
    ]);
    expect(within(report).getByRole("button", { name: t("appflowyImport.report.openSpace", { name: "Studio" }) })).toBeInTheDocument();
    // The report that has spaces has no single "Open".
    expect(within(report).queryByRole("button", { name: t("import.report.open") })).not.toBeInTheDocument();
    expect(within(report).getByText(t("import.report.noWarnings"))).toBeInTheDocument();
  });

  it("has the pages the import wrote while it ran, not the ones the list had before", async () => {
    const { user } = await renderInterop(
      { spaces: [PERSONAL] },
      {
        // The import makes two spaces and fills them, as the core does while the shell waits for it.
        methods: (memory) => ({
          importRun: () => {
            const client = new CoreClient(memory);
            return (async () => {
              const first = await client.spaceCreate("Home base");
              const second = await client.spaceCreate("Elsewhere");
              await client.pageCreate({ spaceId: first.id, parentId: null, title: "One" });
              await client.pageCreate({ spaceId: first.id, parentId: null, title: "Two" });
              await client.pageCreate({ spaceId: second.id, parentId: null, title: "Three" });
              return importReport({ pages: 3, spaceIds: [first.id, second.id] });
            })();
          },
        }),
      },
    );
    const dialog = await openAppFlowyImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("appflowyImport.choose") }));
    const report = await screen.findByRole("dialog", { name: t("import.report.title") });
    const list = within(report).getByRole("region", { name: t("appflowyImport.report.spaces") });
    expect(within(list).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      `Home base${tp("spaces.pageCount", 2)}${t("appflowyImport.report.open")}`,
      `Elsewhere${tp("spaces.pageCount", 1)}${t("appflowyImport.report.open")}`,
    ]);
  });

  it("opens a space: the dialog closes and the sidebar is in that space", async () => {
    const importRun = vi.fn<ImportRun>(() => Promise.resolve(importReport()));
    const { user, ids, store } = await setup(importRun);
    importRun.mockResolvedValue(twoSpaces(ids));
    const dialog = await openAppFlowyImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("appflowyImport.choose") }));
    const report = await screen.findByRole("dialog", { name: t("import.report.title") });
    await user.click(within(report).getByRole("button", { name: t("appflowyImport.report.openSpace", { name: "Studio" }) }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(store.getState().currentSpaceId).toBe(ids.space("Studio"));
    expect(screen.getByRole("button", { description: t("spaces.switchTitle") })).toHaveTextContent("Studio");
    expect(await screen.findByRole("treeitem", { name: "Shots" })).toBeInTheDocument();
  });

  it("lists the warnings as the Import dialog does, and closes with Close", async () => {
    const importRun = vi.fn<ImportRun>(() => Promise.resolve(importReport()));
    const { user, ids } = await setup(importRun);
    importRun.mockResolvedValue({
      ...twoSpaces(ids),
      warnings: [
        { path: "Studio/Page 1", code: "remoteImage" },
        { path: "Studio/Page 2", code: "remoteImage" },
        { path: "Work/Board", code: "unsupportedContent", detail: "block type" },
      ],
    });
    const dialog = await openAppFlowyImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("appflowyImport.choose") }));
    const report = await screen.findByRole("dialog", { name: t("import.report.title") });
    const groups = Array.from(report.querySelectorAll("details")).map((group) => group.querySelector("summary")?.textContent);
    expect(groups).toEqual([
      t("import.report.group", { label: t("import.warning.remoteImage"), count: 2 }),
      t("import.report.group", { label: t("import.warning.unsupportedContent"), count: 1 }),
    ]);
    await user.click(within(report).getByRole("button", { name: t("dialog.close") }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("says nothing was imported, with no spaces to open, when the backup was empty", async () => {
    const { user } = await setup(vi.fn<ImportRun>(() => Promise.resolve(importReport({ pages: 0, spaceIds: [] }))));
    const dialog = await openAppFlowyImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("appflowyImport.choose") }));
    const report = await screen.findByRole("dialog", { name: t("import.report.title") });
    expect(within(report).getByText(t("import.report.none"))).toBeInTheDocument();
    expect(within(report).queryByRole("region", { name: t("appflowyImport.report.spaces") })).not.toBeInTheDocument();
  });

  it("lists the new spaces of a report that has some whichever dialog started it", async () => {
    const importRun = vi.fn<ImportRun>(() => Promise.resolve(importReport()));
    const { user, ids } = await setup(importRun);
    importRun.mockResolvedValue(twoSpaces(ids));
    await user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
    await user.click(await screen.findByRole("menuitem", { name: t("interop.menu.importSpace") }));
    const dialog = await screen.findByRole("dialog", { name: t("import.title") });
    await user.click(within(dialog).getByRole("button", { name: t("import.zip") }));
    const report = await screen.findByRole("dialog", { name: t("import.report.title") });
    expect(within(report).getByRole("region", { name: t("appflowyImport.report.spaces") })).toBeInTheDocument();
    // The Import dialog's own call keeps its destination: the page or space it was opened for.
    expect(importRun.mock.calls[0]?.[0].destination).toBeUndefined();
  });
});

describe("with the in-memory core and its canned AppFlowy backup", () => {
  async function setupMemory() {
    return renderInterop(
      { spaces: [PERSONAL], importSources: demoImportSources() },
      { methods: (memory) => ({ importRun: (options) => memory.importRun(options) }) },
    );
  }

  it("creates a space for each space of the backup, lists them with their pages, and opens one", async () => {
    const { user, store } = await setupMemory();
    const dialog = await openAppFlowyImport(user);
    await user.click(within(dialog).getByRole("button", { name: t("appflowyImport.choose") }));
    const report = await screen.findByRole("dialog", { name: t("import.report.title") });

    const list = within(report).getByRole("region", { name: t("appflowyImport.report.spaces") });
    expect(within(list).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      `🏠Personal${tp("spaces.pageCount", 7)}${t("appflowyImport.report.open")}`,
      `💼Work${tp("spaces.pageCount", 2)}${t("appflowyImport.report.open")}`,
    ]);
    // The backup has an image on the web: the report says it was kept as a link.
    expect(
      within(report).getByText(t("import.report.group", { label: t("import.warning.remoteImage"), count: 1 })),
    ).toBeInTheDocument();
    // The workspace has the first space and the two new ones.
    expect(store.getState().spaces.map((space) => space.name)).toEqual(["Personal", "Personal", "Work"]);

    await user.click(within(report).getByRole("button", { name: t("appflowyImport.report.openSpace", { name: "Work" }) }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(store.getState().currentSpaceId).toBe(store.getState().spaces[2]?.id);
    await waitFor(() => {
      expect(outline()).toEqual(["Meeting notes", "Reading list"]);
    });
  });
});
