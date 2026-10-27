import { beforeEach, describe, expect, it, vi } from "vitest";

import { CoreError } from "./errors";
import { TauriTransport } from "./tauri";

// Stand-in for `@tauri-apps/api/core`: `invoke` is a spy (the shell's commands are not there).
const tauri = vi.hoisted(() => {
  class MockChannel<T> {
    readonly onmessage: (message: T) => void;
    constructor(onmessage: (message: T) => void) {
      this.onmessage = onmessage;
    }
  }
  return { invoke: vi.fn<(command: string, args?: unknown) => Promise<unknown>>(), MockChannel };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke, Channel: tauri.MockChannel }));

const space = "22222222-2222-4222-8222-222222222222";
const page = "33333333-3333-4333-8333-333333333333";

const summary = {
  bytes: 1024,
  documents: 3,
  settings: 2,
  createdAt: 1_790_000_000_000,
  accountId: "44444444-4444-4444-8444-444444444444",
  chunks: 0,
  chunkBytes: 0,
};

beforeEach(() => {
  tauri.invoke.mockReset();
  tauri.invoke.mockResolvedValue(undefined);
});

describe("TauriTransport.exportRun", () => {
  it("invokes export_run with the options as one argument and returns what the shell wrote", async () => {
    const result = {
      path: "C:/exports/Space.zip",
      pages: 4,
      attachments: 2,
      warnings: [{ code: "attachmentUnreadable", attachmentId: "55555555-5555-4555-8555-555555555555" }],
    };
    tauri.invoke.mockResolvedValueOnce(result);
    const options = { spaceId: space, pageId: page, includeChildren: true, format: "markdown" } as const;

    await expect(new TauriTransport().exportRun(options)).resolves.toEqual(result);

    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("export_run", { options });
  });

  it("rejects a cancelled dialog with InvalidInput for the field path", async () => {
    tauri.invoke.mockRejectedValueOnce({
      code: "InvalidInput",
      message: "no file was chosen",
      details: { field: "path" },
    });
    const error: unknown = await new TauriTransport()
      .exportRun({ spaceId: space, includeChildren: false, format: "html" })
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect(error).toMatchObject({ code: "InvalidInput", details: { field: "path" } });
  });

  it("reports an answer that is not an export result as Internal", async () => {
    for (const bad of [undefined, null, "path", { path: "a.zip" }, { path: 1, pages: 1, attachments: 0, warnings: [] }]) {
      tauri.invoke.mockResolvedValueOnce(bad);
      await expect(
        new TauriTransport().exportRun({ spaceId: space, includeChildren: false, format: "markdown" }),
        JSON.stringify(bad),
      ).rejects.toMatchObject({ code: "Internal" });
    }
  });
});

describe("TauriTransport.importRun", () => {
  it("invokes import_run with the options as one argument and returns the report", async () => {
    const report = {
      rootPageIds: [page],
      pages: 12,
      attachments: 3,
      propertiesCreated: 1,
      warnings: [{ path: "a.md", code: "unresolvedLink", detail: "b.md" }],
      moreWarnings: 0,
    };
    tauri.invoke.mockResolvedValueOnce(report);
    const options = { spaceId: space, parentId: page, source: "zip" } as const;

    await expect(new TauriTransport().importRun(options)).resolves.toEqual(report);

    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("import_run", { options });
  });

  it("passes the destination, and sends no space or page for new spaces (ADR-0039)", async () => {
    const report = {
      rootPageIds: [page],
      pages: 5,
      attachments: 1,
      propertiesCreated: 0,
      moreWarnings: 0,
      spaceIds: [space, "66666666-6666-4666-8666-666666666666"],
    };
    tauri.invoke.mockResolvedValue(report);

    const intoSpaces = { destination: "newSpaces", spaceId: space, parentId: page, source: "zip" } as const;
    await expect(new TauriTransport().importRun(intoSpaces)).resolves.toEqual(report);
    expect(tauri.invoke).toHaveBeenLastCalledWith("import_run", {
      options: { destination: "newSpaces", source: "zip" },
    });

    const intoPage = { destination: "page", spaceId: space, parentId: page, source: "folder" } as const;
    await new TauriTransport().importRun(intoPage);
    expect(tauri.invoke).toHaveBeenLastCalledWith("import_run", { options: intoPage });
  });

  it("reports a report whose spaceIds are not a list as Internal", async () => {
    tauri.invoke.mockResolvedValueOnce({
      rootPageIds: [],
      pages: 0,
      attachments: 0,
      propertiesCreated: 0,
      moreWarnings: 0,
      spaceIds: "x",
    });
    await expect(
      new TauriTransport().importRun({ destination: "newSpaces", spaceId: space, source: "zip" }),
    ).rejects.toMatchObject({ code: "Internal" });
  });

  it("accepts a report without warnings (the shell leaves the list out when it is empty)", async () => {
    const report = { rootPageIds: [], pages: 0, attachments: 0, propertiesCreated: 0, moreWarnings: 0 };
    tauri.invoke.mockResolvedValueOnce(report);
    await expect(new TauriTransport().importRun({ spaceId: space, source: "files" })).resolves.toEqual(report);
  });

  it("normalises failures into CoreErrors and reports a malformed answer as Internal", async () => {
    tauri.invoke.mockRejectedValueOnce({ code: "Locked", message: "the workspace is locked" });
    await expect(new TauriTransport().importRun({ spaceId: space, source: "folder" })).rejects.toMatchObject({
      code: "Locked",
    });
    tauri.invoke.mockResolvedValueOnce({ pages: 1 });
    await expect(new TauriTransport().importRun({ spaceId: space, source: "folder" })).rejects.toMatchObject({
      code: "Internal",
    });
  });
});

describe("TauriTransport.saveTextFile", () => {
  it("invokes save_text_file with the suggested name and the text and returns the path", async () => {
    tauri.invoke.mockResolvedValueOnce("C:/exports/Tasks.csv");
    await expect(new TauriTransport().saveTextFile("Tasks.csv", "\uFEFFName\r\n")).resolves.toBe("C:/exports/Tasks.csv");
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("save_text_file", {
      suggestedName: "Tasks.csv",
      text: "\uFEFFName\r\n",
    });
  });

  it("rejects when the shell answers something that is not a path, or fails", async () => {
    tauri.invoke.mockResolvedValueOnce(undefined);
    await expect(new TauriTransport().saveTextFile("a.csv", "x")).rejects.toMatchObject({ code: "Internal" });
    tauri.invoke.mockRejectedValueOnce({ code: "InvalidInput", message: "too large", details: { field: "text" } });
    await expect(new TauriTransport().saveTextFile("a.csv", "x")).rejects.toMatchObject({
      code: "InvalidInput",
      details: { field: "text" },
    });
  });
});

describe("TauriTransport scheduled backups", () => {
  it("backupFolderChoose invokes backup_folder_choose and returns the folder", async () => {
    tauri.invoke.mockResolvedValueOnce("D:/Backups");
    await expect(new TauriTransport().backupFolderChoose()).resolves.toBe("D:/Backups");
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("backup_folder_choose");
  });

  it("backupFolderChoose rejects a cancelled dialog and an answer that is not a path", async () => {
    tauri.invoke.mockRejectedValueOnce({ code: "InvalidInput", message: "no file was chosen", details: { field: "path" } });
    await expect(new TauriTransport().backupFolderChoose()).rejects.toMatchObject({ code: "InvalidInput" });
    tauri.invoke.mockResolvedValueOnce({ path: "D:/Backups" });
    await expect(new TauriTransport().backupFolderChoose()).rejects.toMatchObject({ code: "Internal" });
  });

  it("backupRunNow invokes backup_run_now and returns the summary with the file", async () => {
    tauri.invoke.mockResolvedValueOnce({ summary, path: "D:/Backups/atomik-kb-auto-2026-10-12-1130.atomikkb" });
    await expect(new TauriTransport().backupRunNow()).resolves.toStrictEqual({
      summary,
      path: "D:/Backups/atomik-kb-auto-2026-10-12-1130.atomikkb",
    });
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("backup_run_now");
  });

  it("backupRunNow reports the failure of an attempt and a malformed answer", async () => {
    tauri.invoke.mockRejectedValueOnce({ code: "InvalidInput", message: "no folder", details: { field: "folder" } });
    await expect(new TauriTransport().backupRunNow()).rejects.toMatchObject({
      code: "InvalidInput",
      details: { field: "folder" },
    });
    tauri.invoke.mockResolvedValueOnce({ path: "x" });
    await expect(new TauriTransport().backupRunNow()).rejects.toMatchObject({ code: "Internal" });
  });
});
