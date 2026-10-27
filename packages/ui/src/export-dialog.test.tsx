// The Export dialog of the page menu and of the space menu (ADR-0036): the entries and when they are
// hidden, the formats, "Include sub-pages", the options sent to the shell, the progress line, the toast, a
// cancelled save dialog that says nothing, a failure, and the PDF choice that closes the dialog and prints.

import { CoreError } from "@atomik-kb/core-client";
import type { ExportRunOptions, ExportRunResult, SeedSpace, Transport } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import { MODIFIER } from "./shortcuts";
import { paragraphs } from "./test/editor";
import { exportResult, renderInterop } from "./test/interop";
import { idsOf, openPage } from "./test/page";
import type { User } from "./test/render";

afterEach(() => {
  vi.restoreAllMocks();
});

const SPACES: SeedSpace[] = [
  {
    name: "Personal",
    pages: [
      { title: "Notes", content: paragraphs("Hello"), children: [{ title: "Sub" }] },
      { title: "Alone", content: paragraphs("Nothing below") },
    ],
  },
];

type ExportRun = NonNullable<Transport["exportRun"]>;

function exportMock(result: ExportRunResult = exportResult()) {
  return vi.fn<ExportRun>(() => Promise.resolve(result));
}

async function openPageMenu(user: User, entry: Parameters<typeof t>[0]): Promise<void> {
  await user.click(screen.getByRole("button", { name: t("page.actions") }));
  await user.click(await screen.findByRole("menuitem", { name: t(entry) }));
}

async function openSpaceMenu(user: User, entry: Parameters<typeof t>[0]): Promise<void> {
  await user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
  await user.click(await screen.findByRole("menuitem", { name: t(entry) }));
}

async function exportDialog(name: string): Promise<HTMLElement> {
  return screen.findByRole("dialog", { name: t("export.title.page", { name }) });
}

const toasts = (): HTMLElement => screen.getByRole("log", { name: t("toast.label") });

describe("the entries", () => {
  it("are in the page menu and the space menu when the shell can export and import", async () => {
    const { user } = await renderInterop(
      { spaces: SPACES },
      { methods: { exportRun: exportMock(), importRun: () => Promise.reject(new Error("unused")) } },
    );
    await openPage(user, "Notes");
    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    const menu = await screen.findByRole("menu", { name: t("page.actions") });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      t("page.find"),
      t("comments.menu.page"),
      t("page.copyMarkdown"),
      t("page.copyLink"),
      t("viewLink.menu"), // a personal space's page can be given a view link (ADR-0051)
      t("interop.menu.export"),
      `${t("interop.menu.print")}${MODIFIER}+P`, // the label and the keys that do the same
      t("interop.menu.importPage"),
      t("page.duplicate"),
      t("template.turnInto"),
      t("history.dialog.menu"), // every role can look at the page's history (ADR-0057)
      t("page.info"),
    ]);
    // Print shows its shortcut as a hint at the right of the label, which is not a part of its name.
    const printItem = within(menu).getByRole("menuitem", { name: t("interop.menu.print") });
    expect(printItem).toHaveAccessibleName("Print…");
    expect(printItem).toHaveAttribute("aria-keyshortcuts", expect.stringMatching(/^(Control|Meta)\+P$/));
    expect(within(menu).getByRole("menuitem", { name: t("interop.menu.export") })).not.toHaveAttribute("aria-keyshortcuts");
    await user.keyboard("{Escape}");

    await user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
    const spaceMenu = await screen.findByRole("menu", { name: t("spaces.optionsLabel") });
    const labels = within(spaceMenu).getAllByRole("menuitem").map((item) => item.textContent);
    expect(labels).toContain(t("interop.menu.exportSpace"));
    expect(labels).toContain(t("interop.menu.importSpace"));
  });

  it("are hidden when the shell lacks the capability, but Print stays", async () => {
    const { user } = await renderInterop({ spaces: SPACES });
    await openPage(user, "Notes");
    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    const menu = await screen.findByRole("menu", { name: t("page.actions") });
    expect(within(menu).queryByRole("menuitem", { name: t("interop.menu.export") })).not.toBeInTheDocument();
    expect(within(menu).queryByRole("menuitem", { name: t("interop.menu.importPage") })).not.toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: t("interop.menu.print") })).toBeInTheDocument();
    await user.keyboard("{Escape}");

    await user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
    const spaceMenu = await screen.findByRole("menu", { name: t("spaces.optionsLabel") });
    expect(within(spaceMenu).queryByRole("menuitem", { name: t("interop.menu.exportSpace") })).not.toBeInTheDocument();
    expect(within(spaceMenu).queryByRole("menuitem", { name: t("interop.menu.importSpace") })).not.toBeInTheDocument();
  });
});

describe("the dialog of a page", () => {
  it("offers Markdown, HTML and PDF, and Include sub-pages for a page that has children", async () => {
    const { user } = await renderInterop({ spaces: SPACES }, { methods: { exportRun: exportMock() } });
    await openPage(user, "Notes");
    await openPageMenu(user, "interop.menu.export");
    const dialog = await exportDialog("Notes");

    const formats = within(dialog).getAllByRole("radio");
    expect(formats.map((radio) => radio.parentElement?.textContent)).toEqual([
      t("export.format.markdown"),
      t("export.format.html"),
      t("export.format.pdf"),
    ]);
    expect(within(dialog).getByRole("radio", { name: t("export.format.markdown") })).toBeChecked();
    const subpages = within(dialog).getByRole("checkbox", { name: t("export.subpages") });
    expect(subpages).not.toBeChecked();

    // PDF is one page at a time: no sub-pages for it.
    await user.click(within(dialog).getByRole("radio", { name: t("export.format.pdf") }));
    expect(within(dialog).queryByRole("checkbox", { name: t("export.subpages") })).not.toBeInTheDocument();
    expect(within(dialog).getByRole("radio", { name: t("export.format.pdf") })).toHaveAccessibleDescription(
      t("export.format.pdf.hint"),
    );
    await user.click(within(dialog).getByRole("radio", { name: t("export.format.html") }));
    expect(within(dialog).getByRole("checkbox", { name: t("export.subpages") })).toBeInTheDocument();
  });

  it("does not offer Include sub-pages for a page with none", async () => {
    const { user } = await renderInterop({ spaces: SPACES }, { methods: { exportRun: exportMock() } });
    await openPage(user, "Alone");
    await openPageMenu(user, "interop.menu.export");
    const dialog = await exportDialog("Alone");
    expect(within(dialog).queryByRole("checkbox", { name: t("export.subpages") })).not.toBeInTheDocument();
  });

  it("sends the page, the format and the sub-pages to the shell, then closes and says what was written", async () => {
    const exportRun = exportMock(exportResult({ path: "C:\\Users\\Ada\\Notes.zip", pages: 2 }));
    const { user, memory } = await renderInterop({ spaces: SPACES }, { methods: { exportRun } });
    const ids = await idsOf(memory);
    await openPage(user, "Notes");
    await openPageMenu(user, "interop.menu.export");
    const dialog = await exportDialog("Notes");

    await user.click(within(dialog).getByRole("radio", { name: t("export.format.html") }));
    await user.click(within(dialog).getByRole("checkbox", { name: t("export.subpages") }));
    await user.click(within(dialog).getByRole("button", { name: t("export.submit") }));

    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(exportRun).toHaveBeenCalledTimes(1);
    expect(exportRun).toHaveBeenCalledWith({
      spaceId: ids.space("Personal"),
      pageId: ids.page("Notes"),
      includeChildren: true,
      format: "html",
    } satisfies ExportRunOptions);
    expect(toasts()).toHaveTextContent("Exported 2 pages to Notes.zip.");
    // Focus is back on the page menu's button.
    expect(screen.getByRole("button", { name: t("page.actions") })).toHaveFocus();
  });

  it("exports the page alone by default, as Markdown, and counts what was left out", async () => {
    const left = { code: "attachmentNotDownloaded", attachmentId: "a" } as const;
    const exportRun = exportMock(exportResult({ path: "/home/ada/Alone.md", pages: 1, warnings: [left, left] }));
    const { user, memory } = await renderInterop({ spaces: SPACES }, { methods: { exportRun } });
    const ids = await idsOf(memory);
    await openPage(user, "Notes");
    await openPageMenu(user, "interop.menu.export");
    await user.click(within(await exportDialog("Notes")).getByRole("button", { name: t("export.submit") }));

    await waitFor(() => {
      expect(exportRun).toHaveBeenCalledWith({
        spaceId: ids.space("Personal"),
        pageId: ids.page("Notes"),
        includeChildren: false,
        format: "markdown",
      });
    });
    await waitFor(() => {
      expect(toasts()).toHaveTextContent("Exported 1 page to Alone.md. 2 items were left out.");
    });
  });

  it("shows the progress of the export and cannot be dismissed while it runs", async () => {
    let finish: (value: ExportRunResult) => void = () => undefined;
    const exportRun = vi.fn<ExportRun>(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const { user, emit } = await renderInterop({ spaces: SPACES }, { methods: { exportRun } });
    await openPage(user, "Notes");
    await openPageMenu(user, "interop.menu.export");
    const dialog = await exportDialog("Notes");
    await user.click(within(dialog).getByRole("button", { name: t("export.submit") }));

    // Before the first event the shell is asking where to save.
    expect(await within(dialog).findByRole("status")).toHaveTextContent(t("export.preparing"));
    expect(within(dialog).getByRole("button", { name: t("export.working") })).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: t("dialog.cancel") })).toBeDisabled();

    act(() => {
      emit({ type: "ExportProgress", data: { exportId: 1, done: 2, total: 5 } });
    });
    expect(within(dialog).getByRole("status")).toHaveTextContent(t("export.progress", { done: 2, total: 5 }));

    await user.keyboard("{Escape}");
    expect(screen.getByRole("dialog")).toBeInTheDocument();

    await act(async () => {
      finish(exportResult());
      await Promise.resolve();
    });
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
  });

  it("says nothing when the save dialog is cancelled, and stays ready for another try", async () => {
    const exportRun = vi.fn<ExportRun>(() =>
      Promise.reject(new CoreError({ code: "InvalidInput", message: "no file was chosen", details: { field: "path" } })),
    );
    const { user } = await renderInterop({ spaces: SPACES }, { methods: { exportRun } });
    await openPage(user, "Notes");
    await openPageMenu(user, "interop.menu.export");
    const dialog = await exportDialog("Notes");
    await user.click(within(dialog).getByRole("button", { name: t("export.submit") }));

    await waitFor(() => {
      expect(exportRun).toHaveBeenCalledTimes(1);
      expect(within(dialog).getByRole("button", { name: t("export.submit") })).toBeEnabled();
    });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("status")).not.toBeInTheDocument();
    expect(toasts()).toBeEmptyDOMElement();
    expect(screen.getByRole("dialog")).toBe(dialog);
  });

  it("shows why an export failed, in the dialog", async () => {
    const exportRun = vi.fn<ExportRun>(() => Promise.reject(new CoreError({ code: "Storage", message: "disk full" })));
    const { user } = await renderInterop({ spaces: SPACES }, { methods: { exportRun } });
    await openPage(user, "Notes");
    await openPageMenu(user, "interop.menu.export");
    const dialog = await exportDialog("Notes");
    await user.click(within(dialog).getByRole("button", { name: t("export.submit") }));

    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("export.error", { message: "disk full" }));
    expect(toasts()).toBeEmptyDOMElement();
    await user.click(within(dialog).getByRole("button", { name: t("dialog.cancel") }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("closes with Escape and gives focus back", async () => {
    const { user } = await renderInterop({ spaces: SPACES }, { methods: { exportRun: exportMock() } });
    await openPage(user, "Notes");
    await openPageMenu(user, "interop.menu.export");
    await exportDialog("Notes");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("page.actions") })).toHaveFocus();
  });

  it("closes and starts print mode for the PDF choice, without calling the shell's export", async () => {
    const exportRun = exportMock();
    const print = vi.fn<() => void>();
    vi.spyOn(window, "print").mockImplementation(print);
    const { user, store } = await renderInterop({ spaces: SPACES }, { methods: { exportRun } });
    await openPage(user, "Notes");
    await openPageMenu(user, "interop.menu.export");
    const dialog = await exportDialog("Notes");
    await user.click(within(dialog).getByRole("radio", { name: t("export.format.pdf") }));
    await user.click(within(dialog).getByRole("button", { name: t("export.submit") }));

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(store.getState().printing).toBe(true);
    await waitFor(() => {
      expect(print).toHaveBeenCalledTimes(1);
    });
    expect(exportRun).not.toHaveBeenCalled();
    act(() => {
      window.dispatchEvent(new Event("afterprint"));
    });
    expect(store.getState().printing).toBe(false);
  });
});

describe("the dialog of a space", () => {
  it("offers Markdown and HTML only and exports every page of the space", async () => {
    const exportRun = exportMock(exportResult({ path: "C:\\x\\Personal.zip", pages: 3 }));
    const { user, memory } = await renderInterop({ spaces: SPACES }, { methods: { exportRun } });
    const ids = await idsOf(memory);
    await openSpaceMenu(user, "interop.menu.exportSpace");
    const dialog = await screen.findByRole("dialog", { name: t("export.title.space", { name: "Personal" }) });

    expect(within(dialog).getAllByRole("radio").map((radio) => radio.parentElement?.textContent)).toEqual([
      t("export.format.markdown"),
      t("export.format.html"),
    ]);
    expect(within(dialog).queryByRole("checkbox")).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("export.submit") }));

    await waitFor(() => {
      expect(exportRun).toHaveBeenCalledTimes(1);
    });
    const options = exportRun.mock.calls[0]?.[0];
    expect(options).toMatchObject({ spaceId: ids.space("Personal"), includeChildren: true, format: "markdown" });
    expect(options?.pageId).toBeUndefined();
    await waitFor(() => {
      expect(toasts()).toHaveTextContent("Exported 3 pages to Personal.zip.");
    });
  });
});
