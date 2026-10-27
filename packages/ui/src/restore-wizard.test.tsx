// The restore wizard (ADR-0059, `docs/architecture/drive-backup.md` section 6): the source, the list of a folder's or Drive's
// backups, the download with its progress and Cancel, the restore and the kit dialog behind it. The shell's `drive_*` commands and
// `backup_folder_list` are small fakes here (a `DriveApi` of mocks, a `backupFolderList` stub) over a `MemoryTransport` that holds
// the backup files on its fake disk; the file source itself is covered by `restore.test.tsx`.

import { CoreClient, CoreError, MemoryTransport, kitWordsFromEntropy } from "@atomik-kb/core-client";
import type {
  BackupFileInfo,
  BackupFolderListing,
  BackupResult,
  DriveApi,
  DriveProgress,
  DriveStatus,
  Transport,
} from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { formatBytes, formatTimestamp } from "./format";
import { t } from "./i18n";
import { NOW, findMainNavigation, renderWith } from "./test/render";
import type { User } from "./test/render";

const FOLDER = "C:\\Users\\you\\Backups";
const OLDER = `${FOLDER}\\atomik-kb-auto-2026-10-01-0900.atomikkb`;
const NEWER = `${FOLDER}\\atomik-kb-auto-2026-10-02-0900.atomikkb`;
const STAGING = "C:\\Users\\you\\AppData\\Local\\atomik-kb\\restore-staging\\a1b2c3.atomikkb";

function info(id: string, name: string, modifiedAt: number, size: number): BackupFileInfo {
  return { id, name, size, modifiedAt };
}

const FOLDER_OLDER = info(OLDER, "atomik-kb-auto-2026-10-01-0900.atomikkb", Date.UTC(2026, 9, 1, 9), 10_240);
const FOLDER_NEWER = info(NEWER, "atomik-kb-auto-2026-10-02-0900.atomikkb", Date.UTC(2026, 9, 2, 9), 12_900);
const DRIVE_OLDER = info("drive-file-1", "atomik-kb-auto-2026-10-03-0900.atomikkb", Date.UTC(2026, 9, 3, 9), 4096);
const DRIVE_NEWER = info("drive-file-2", "atomik-kb-auto-2026-10-04-0900.atomikkb", Date.UTC(2026, 9, 4, 9), 8192);

const DISCONNECTED: DriveStatus = { client: "builtIn", connected: false, needsReconnect: false };
const CONNECTED: DriveStatus = { client: "builtIn", connected: true, account: "ben@example.com", needsReconnect: false };

/** A workspace with a kit, backed up under three names of a fake disk: two in a folder, one where Drive's download lands. */
async function disk() {
  const kitKey = new Uint8Array(32).fill(7);
  const source = new MemoryTransport({
    now: () => NOW,
    workspaceName: "Research",
    spaces: [{ name: "Personal", pages: [{ title: "Welcome" }, { title: "Journal" }] }],
    security: { kitCreatedAt: NOW, kitKey },
  });
  await source.backupExport(NEWER);
  const bytes = source.files.get(NEWER) ?? new Uint8Array();
  const files = new Map<string, Uint8Array>([
    [NEWER, bytes],
    [OLDER, bytes],
    [STAGING, bytes],
  ]);
  return { files, words: await kitWordsFromEntropy(kitKey) };
}

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve: (value: T) => void = () => undefined;
  let reject: (reason: unknown) => void = () => undefined;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** A failure of a Drive command as the shell reports it: a generic message and the reason in `details`. */
function driveError(reason: string): CoreError {
  return new CoreError({ code: "Internal", message: "drive failed", details: { reason } });
}

/** A cancelled dialog: `InvalidInput` for the field `path`. */
function noPath(message = "no file was chosen"): CoreError {
  return new CoreError({ code: "InvalidInput", message, details: { field: "path" } });
}

/** A `DriveApi` made of mocks: connected or not as `status` says, `backups` listed, anything else a test overrides. */
function fakeDrive(status: DriveStatus, backups: BackupFileInfo[] = [DRIVE_OLDER, DRIVE_NEWER]) {
  const mocks = {
    status: vi.fn<DriveApi["status"]>(() => Promise.resolve(status)),
    connect: vi.fn<DriveApi["connect"]>(() => Promise.resolve(CONNECTED)),
    disconnect: vi.fn<DriveApi["disconnect"]>(() => Promise.resolve(DISCONNECTED)),
    setClient: vi.fn<DriveApi["setClient"]>(() => Promise.resolve(status)),
    listBackups: vi.fn<DriveApi["listBackups"]>(() => Promise.resolve(backups)),
    download: vi.fn<DriveApi["download"]>(() => Promise.resolve(STAGING)),
    cancel: vi.fn<DriveApi["cancel"]>(() => Promise.resolve()),
  };
  return mocks;
}

type FolderList = (path?: string) => Promise<BackupFolderListing>;

interface Shell {
  drive?: DriveApi;
  backupFolderList?: FolderList;
}

/** A fresh installation whose disk holds the backups, over the shell's extra commands; records the restores it was asked for. */
function installation(files: Map<string, Uint8Array>, shell: Shell = {}) {
  const memory = new MemoryTransport({ now: () => NOW, files });
  const imports: { path: string | undefined; kit: string | undefined }[] = [];
  const transport: Transport = {
    dispatch: (request) => memory.dispatch(request),
    subscribe: (onEvent) => memory.subscribe(onEvent),
    coreState: () => memory.coreState(),
    docs: memory.docs,
    lock: () => memory.lock(),
    unlock: (passphrase) => memory.unlock(passphrase),
    backupExport: (path) => memory.backupExport(path),
    backupImport: (path, kit): Promise<BackupResult> => {
      imports.push({ path, kit });
      return memory.backupImport(path, kit);
    },
    ...shell,
  };
  return { memory, imports, ...renderWith(transport) };
}

async function openWizard(user: User): Promise<HTMLElement> {
  await screen.findByRole("heading", { name: t("onboarding.title") });
  await user.click(screen.getByRole("button", { name: t("onboarding.restore.button") }));
  return screen.findByRole("dialog", { name: t("restore.wizard.title") });
}

function button(wizard: HTMLElement, name: string): HTMLElement {
  return within(wizard).getByRole("button", { name });
}

async function findStep(wizard: HTMLElement, name: string): Promise<HTMLElement> {
  return within(wizard).findByRole("heading", { name });
}

async function typeKit(user: User, words: string[]): Promise<HTMLElement> {
  const dialog = await screen.findByRole("dialog", { name: t("restoreKit.title") });
  const field = within(dialog).getByRole("textbox", { name: t("restoreKit.label") });
  await user.click(field);
  await user.paste(words.join(" "));
  return dialog;
}

describe("restore wizard: the source", () => {
  it("offers the file alone where the shell lists no folders and has no Drive", async () => {
    const { files } = await disk();
    const { user } = installation(files);
    const wizard = await openWizard(user);

    const step = within(wizard).getByRole("heading", { name: t("restore.wizard.source.title") });
    expect(step).toHaveFocus();
    expect(button(wizard, t("restore.wizard.source.file"))).toBeEnabled();
    expect(within(wizard).queryByRole("button", { name: t("restore.wizard.source.folder") })).not.toBeInTheDocument();
    expect(within(wizard).queryByText(t("restore.wizard.source.drive"))).not.toBeInTheDocument();
  });

  it("restores the file the shell's dialog gives, as it did before the wizard", async () => {
    const { files, words } = await disk();
    const { user, imports } = installation(files);
    const wizard = await openWizard(user);
    await user.click(button(wizard, t("restore.wizard.source.file")));
    await typeKit(user, words);
    await user.click(screen.getByRole("button", { name: t("restoreKit.submit") }));
    await findMainNavigation();
    // The shell asked for the file (no path), then the dialog restored the one it had read.
    expect(imports[0]).toEqual({ path: undefined, kit: undefined });
  });

  it("closes on Escape and on Cancel when nothing runs, and gives focus back to the button", async () => {
    const { files } = await disk();
    const { user } = installation(files);
    await openWizard(user);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("onboarding.restore.button") })).toHaveFocus();

    const wizard = await openWizard(user);
    await user.click(button(wizard, t("dialog.cancel")));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("onboarding.restore.button") })).toHaveFocus();
  });
});

describe("restore wizard: a folder of backups", () => {
  it("lists the backups newest first, the newest selected, and restores the one chosen with the kit", async () => {
    const { files, words } = await disk();
    // Oldest first on purpose: the wizard orders them itself.
    const list = vi.fn<FolderList>(() => Promise.resolve({ folder: FOLDER, backups: [FOLDER_OLDER, FOLDER_NEWER] }));
    const { user, imports } = installation(files, { backupFolderList: list });
    const wizard = await openWizard(user);
    await user.click(button(wizard, t("restore.wizard.source.folder")));

    const step = await findStep(wizard, t("restore.wizard.list.title"));
    expect(step).toHaveFocus();
    expect(list.mock.calls).toEqual([[undefined]]);
    expect(within(wizard).getByText(t("restore.wizard.list.folder", { folder: FOLDER }))).toBeInTheDocument();
    const group = within(wizard).getByRole("radiogroup", { name: t("restore.wizard.list.title") });
    const radios = within(group).getAllByRole("radio");
    expect(radios).toHaveLength(2);
    expect(radios[0]).toHaveAccessibleName(FOLDER_NEWER.name);
    expect(radios[1]).toHaveAccessibleName(FOLDER_OLDER.name);
    expect(radios[0]).toBeChecked();
    expect(radios[1]).not.toBeChecked();
    // Date and time of the person's clock, and the size, describe each backup.
    expect(radios[0]).toHaveAccessibleDescription(
      `${formatTimestamp(FOLDER_NEWER.modifiedAt)?.text ?? ""} ${formatBytes(FOLDER_NEWER.size)}`,
    );

    await user.click(radios[1] as HTMLElement);
    expect(radios[1]).toBeChecked();
    await user.click(button(wizard, t("restore.wizard.list.restore")));

    // A folder's backup is used in place: its path goes to the restore, and again with the kit, with no second file dialog.
    await typeKit(user, words);
    await user.click(screen.getByRole("button", { name: t("restoreKit.submit") }));
    await findMainNavigation();
    expect(imports).toEqual([
      { path: OLDER, kit: undefined },
      { path: OLDER, kit: words.join(" ") },
    ]);
  });

  it("chooses another folder from the list, and keeps the list when that dialog is cancelled", async () => {
    const { files } = await disk();
    const other = "C:\\Elsewhere";
    const list = vi
      .fn<FolderList>()
      .mockResolvedValueOnce({ folder: FOLDER, backups: [FOLDER_NEWER] })
      .mockResolvedValueOnce({ folder: other, backups: [FOLDER_OLDER] })
      .mockRejectedValueOnce(noPath("no folder was chosen"));
    const { user } = installation(files, { backupFolderList: list });
    const wizard = await openWizard(user);
    await user.click(button(wizard, t("restore.wizard.source.folder")));
    await findStep(wizard, t("restore.wizard.list.title"));
    expect(within(wizard).getAllByRole("radio")).toHaveLength(1);

    await user.click(button(wizard, t("restore.wizard.list.chooseAnother")));
    expect(await within(wizard).findByText(t("restore.wizard.list.folder", { folder: other }))).toBeInTheDocument();
    const radios = within(wizard).getAllByRole("radio");
    expect(radios).toHaveLength(1);
    expect(radios[0]).toHaveAccessibleName(FOLDER_OLDER.name);

    await user.click(button(wizard, t("restore.wizard.list.chooseAnother")));
    expect(await within(wizard).findByRole("status")).toHaveTextContent(t("restore.wizard.folderCancelled"));
    expect(within(wizard).getByText(t("restore.wizard.list.folder", { folder: other }))).toBeInTheDocument();
    expect(within(wizard).getAllByRole("radio")).toHaveLength(1);
  });

  it("says so when no folder was chosen, and when the folder cannot be read", async () => {
    const { files } = await disk();
    const list = vi
      .fn<FolderList>()
      .mockRejectedValueOnce(noPath("no folder was chosen"))
      .mockRejectedValueOnce(noPath("the folder cannot be read"));
    const { user } = installation(files, { backupFolderList: list });
    const wizard = await openWizard(user);

    await user.click(button(wizard, t("restore.wizard.source.folder")));
    expect(await within(wizard).findByRole("status")).toHaveTextContent(t("restore.wizard.folderCancelled"));
    expect(within(wizard).getByRole("heading", { name: t("restore.wizard.source.title") })).toBeInTheDocument();

    await user.click(button(wizard, t("restore.wizard.source.folder")));
    expect(await within(wizard).findByRole("alert")).toHaveTextContent(
      t("restore.wizard.error.folder", { message: "the folder cannot be read" }),
    );
    expect(button(wizard, t("restore.wizard.source.folder"))).toBeEnabled();
  });

  it("says so when the folder has no backups, and goes back to the source from there", async () => {
    const { files } = await disk();
    const list = vi.fn<FolderList>(() => Promise.resolve({ folder: FOLDER, backups: [] }));
    const { user } = installation(files, { backupFolderList: list });
    const wizard = await openWizard(user);
    await user.click(button(wizard, t("restore.wizard.source.folder")));

    await findStep(wizard, t("restore.wizard.list.title"));
    expect(within(wizard).getByRole("status")).toHaveTextContent(t("restore.wizard.list.empty.folder"));
    expect(within(wizard).queryByRole("radio")).not.toBeInTheDocument();
    expect(button(wizard, t("restore.wizard.list.restore"))).toBeDisabled();

    await user.click(button(wizard, t("onboarding.back")));
    expect(await findStep(wizard, t("restore.wizard.source.title"))).toHaveFocus();
  });

  it("says so when a listed backup is damaged or gone, and stays on the list", async () => {
    const { files } = await disk();
    const damaged = (files.get(NEWER) ?? new Uint8Array()).slice();
    const middle = Math.floor(damaged.length / 2);
    damaged[middle] = (damaged[middle] ?? 0) ^ 0x01;
    files.set(NEWER, damaged);
    files.delete(OLDER);
    const list = vi.fn<FolderList>(() => Promise.resolve({ folder: FOLDER, backups: [FOLDER_NEWER, FOLDER_OLDER] }));
    const { user, memory } = installation(files, { backupFolderList: list });
    const wizard = await openWizard(user);
    await user.click(button(wizard, t("restore.wizard.source.folder")));
    await findStep(wizard, t("restore.wizard.list.title"));

    await user.click(button(wizard, t("restore.wizard.list.restore")));
    expect(await within(wizard).findByRole("alert")).toHaveTextContent(t("onboarding.restore.error.damaged"));
    expect(within(wizard).getAllByRole("radio")).toHaveLength(2);

    await user.click(within(wizard).getAllByRole("radio")[1] as HTMLElement);
    await user.click(button(wizard, t("restore.wizard.list.restore")));
    expect(await within(wizard).findByRole("alert")).toHaveTextContent(t("restore.wizard.error.missing"));
    expect(button(wizard, t("restore.wizard.list.restore"))).toBeEnabled();
    expect(screen.queryByRole("dialog", { name: t("restoreKit.title") })).not.toBeInTheDocument();
    await expect(new CoreClient(memory).workspaceGet()).resolves.toBeNull();
  });
});

describe("restore wizard: Google Drive, connecting", () => {
  it("does not offer Drive when the build has no Google client", async () => {
    const { files } = await disk();
    const drive = fakeDrive({ client: "none", connected: false, needsReconnect: false });
    const { user } = installation(files, { drive });
    const wizard = await openWizard(user);
    await waitFor(() => {
      expect(drive.status).toHaveBeenCalled();
    });
    await act(() => Promise.resolve());
    expect(within(wizard).queryByText(t("restore.wizard.source.drive"))).not.toBeInTheDocument();
    expect(within(wizard).queryByRole("button", { name: t("restore.wizard.drive.connect") })).not.toBeInTheDocument();
  });

  it("connects, waits for the browser with a way to cancel, and lists the backups of the folder in Drive", async () => {
    const { files } = await disk();
    const drive = fakeDrive(DISCONNECTED);
    const signedIn = deferred<DriveStatus>();
    drive.connect.mockReturnValue(signedIn.promise);
    const { user } = installation(files, { drive });
    const wizard = await openWizard(user);

    await user.click(await within(wizard).findByRole("button", { name: t("restore.wizard.drive.connect") }));
    expect(await within(wizard).findByRole("status")).toHaveTextContent(t("restore.wizard.drive.waiting"));
    // Nothing else starts while the browser is open; Escape does not close the wizard either.
    expect(button(wizard, t("restore.wizard.source.file"))).toBeDisabled();
    expect(button(wizard, t("dialog.cancel"))).toBeDisabled();
    await user.keyboard("{Escape}");
    expect(screen.getByRole("dialog", { name: t("restore.wizard.title") })).toBeInTheDocument();

    await act(async () => {
      signedIn.resolve(CONNECTED);
      await signedIn.promise;
    });
    const step = await findStep(wizard, t("restore.wizard.list.title"));
    expect(step).toHaveFocus();
    expect(drive.listBackups).toHaveBeenCalledTimes(1);
    expect(within(wizard).getByText(/ben@example\.com/)).toBeInTheDocument();
    const radios = within(wizard).getAllByRole("radio");
    expect(radios[0]).toHaveAccessibleName(DRIVE_NEWER.name);
    expect(radios[0]).toBeChecked();
    expect(radios[1]).toHaveAccessibleName(DRIVE_OLDER.name);
  });

  it("gives the sign-in up on Cancel, with a word about it", async () => {
    const { files } = await disk();
    const drive = fakeDrive(DISCONNECTED);
    const signedIn = deferred<DriveStatus>();
    drive.connect.mockReturnValue(signedIn.promise);
    drive.cancel.mockImplementation(() => {
      signedIn.reject(driveError("cancelled"));
      return Promise.resolve();
    });
    const { user } = installation(files, { drive });
    const wizard = await openWizard(user);

    await user.click(await within(wizard).findByRole("button", { name: t("restore.wizard.drive.connect") }));
    await user.click(await within(wizard).findByRole("button", { name: t("restore.wizard.drive.cancel") }));

    expect(drive.cancel).toHaveBeenCalledTimes(1);
    await waitFor(() => {
      expect(within(wizard).getByText(t("restore.wizard.error.signInCancelled"))).toBeInTheDocument();
    });
    expect(within(wizard).getByRole("button", { name: t("restore.wizard.drive.connect") })).toBeEnabled();
    expect(button(wizard, t("restore.wizard.source.file"))).toBeEnabled();
    expect(drive.listBackups).not.toHaveBeenCalled();
  });

  it.each([
    ["consentDeclined", "restore.wizard.error.consentDeclined", "status"],
    ["browserTimeout", "restore.wizard.error.browserTimeout", "alert"],
    ["denied", "restore.wizard.error.denied", "alert"],
    ["noClient", "restore.wizard.error.noClient", "alert"],
    ["network", "restore.wizard.error.network", "alert"],
  ] as const)("says what went wrong when connecting fails with %s", async (reason, key, role) => {
    const { files } = await disk();
    const drive = fakeDrive(DISCONNECTED);
    drive.connect.mockRejectedValue(driveError(reason));
    const { user } = installation(files, { drive });
    const wizard = await openWizard(user);

    await user.click(await within(wizard).findByRole("button", { name: t("restore.wizard.drive.connect") }));
    expect(await within(wizard).findByRole(role)).toHaveTextContent(t(key));
    expect(within(wizard).getByRole("button", { name: t("restore.wizard.drive.connect") })).toBeEnabled();
  });

  it("asks to connect again when Google no longer accepts the saved sign-in", async () => {
    const { files } = await disk();
    const drive = fakeDrive({ ...CONNECTED, needsReconnect: true });
    const { user } = installation(files, { drive });
    const wizard = await openWizard(user);

    expect(await within(wizard).findByText(t("restore.wizard.drive.reconnect"))).toBeInTheDocument();
    expect(button(wizard, t("restore.wizard.drive.connect"))).toBeEnabled();
    expect(within(wizard).queryByRole("button", { name: t("restore.wizard.drive.show") })).not.toBeInTheDocument();
  });
});

describe("restore wizard: Google Drive, the list and the download", () => {
  it("shows who is connected, lists the backups on request and disconnects from the list", async () => {
    const { files } = await disk();
    const drive = fakeDrive(CONNECTED);
    const { user } = installation(files, { drive });
    const wizard = await openWizard(user);

    expect(await within(wizard).findByText(t("restore.wizard.drive.connectedAs", { account: "ben@example.com" }))).toBeInTheDocument();
    expect(drive.listBackups).not.toHaveBeenCalled();
    await user.click(button(wizard, t("restore.wizard.drive.show")));
    await findStep(wizard, t("restore.wizard.list.title"));
    expect(within(wizard).getAllByRole("radio")).toHaveLength(2);

    await user.click(button(wizard, t("restore.wizard.drive.disconnect")));
    expect(await findStep(wizard, t("restore.wizard.source.title"))).toBeInTheDocument();
    expect(drive.disconnect).toHaveBeenCalledTimes(1);
    expect(await within(wizard).findByRole("button", { name: t("restore.wizard.drive.connect") })).toBeEnabled();
    expect(within(wizard).queryByText(/ben@example\.com/)).not.toBeInTheDocument();
  });

  it("says so when the folder in Drive has no backups", async () => {
    const { files } = await disk();
    const drive = fakeDrive(CONNECTED, []);
    const { user } = installation(files, { drive });
    const wizard = await openWizard(user);
    await user.click(await within(wizard).findByRole("button", { name: t("restore.wizard.drive.show") }));

    await findStep(wizard, t("restore.wizard.list.title"));
    expect(within(wizard).getByRole("status")).toHaveTextContent(t("restore.wizard.list.empty.drive"));
    expect(button(wizard, t("restore.wizard.list.restore"))).toBeDisabled();
  });

  it("downloads the backup with a progress bar, then restores it and asks for the kit with the downloaded path", async () => {
    const { files, words } = await disk();
    const drive = fakeDrive(CONNECTED);
    const downloaded = deferred<string>();
    drive.download.mockReturnValue(downloaded.promise);
    const { user, imports } = installation(files, { drive });
    const wizard = await openWizard(user);
    await user.click(await within(wizard).findByRole("button", { name: t("restore.wizard.drive.show") }));
    await findStep(wizard, t("restore.wizard.list.title"));
    // The older one, not the selected newest.
    await user.click(within(wizard).getAllByRole("radio")[1] as HTMLElement);
    await user.click(button(wizard, t("restore.wizard.list.restore")));

    const step = await findStep(wizard, t("restore.wizard.download.title"));
    expect(step).toHaveFocus();
    expect(drive.download).toHaveBeenCalledTimes(1);
    expect(drive.download.mock.calls[0]?.[0]).toBe(DRIVE_OLDER.id);
    const bar = within(wizard).getByRole("progressbar", { name: t("restore.wizard.download.label") });
    expect(bar).not.toHaveAttribute("value");
    expect(within(wizard).getByText(t("restore.wizard.download.starting"))).toBeInTheDocument();

    const onProgress = drive.download.mock.calls[0]?.[1] as (progress: DriveProgress) => void;
    await act(async () => {
      onProgress({ received: 1024, total: 4096 });
      await Promise.resolve();
    });
    expect(bar).toHaveAttribute("value", "1024");
    expect(bar).toHaveAttribute("max", "4096");
    expect(
      within(wizard).getByText(
        t("restore.wizard.download.progress", { received: formatBytes(1024), total: formatBytes(4096) }),
      ),
    ).toBeInTheDocument();

    // The size may not be known: the bar is then indeterminate and the text says what came.
    await act(async () => {
      onProgress({ received: 2048 });
      await Promise.resolve();
    });
    expect(bar).not.toHaveAttribute("value");
    expect(within(wizard).getByText(t("restore.wizard.download.received", { received: formatBytes(2048) }))).toBeInTheDocument();

    await act(async () => {
      downloaded.resolve(STAGING);
      await downloaded.promise;
    });
    await typeKit(user, words);
    await user.click(screen.getByRole("button", { name: t("restoreKit.submit") }));
    await findMainNavigation();
    // The staging file the shell named is the path of both tries; the kit dialog did not ask for another file.
    expect(imports).toEqual([
      { path: STAGING, kit: undefined },
      { path: STAGING, kit: words.join(" ") },
    ]);
  });

  it("cancels a download on Cancel and goes back to the list with the same backup selected", async () => {
    const { files } = await disk();
    const drive = fakeDrive(CONNECTED);
    const downloaded = deferred<string>();
    drive.download.mockReturnValue(downloaded.promise);
    drive.cancel.mockImplementation(() => {
      downloaded.reject(driveError("cancelled"));
      return Promise.resolve();
    });
    const { user, imports } = installation(files, { drive });
    const wizard = await openWizard(user);
    await user.click(await within(wizard).findByRole("button", { name: t("restore.wizard.drive.show") }));
    await findStep(wizard, t("restore.wizard.list.title"));
    await user.click(within(wizard).getAllByRole("radio")[1] as HTMLElement);
    await user.click(button(wizard, t("restore.wizard.list.restore")));
    await findStep(wizard, t("restore.wizard.download.title"));
    // Nothing closes the wizard while the download runs.
    await user.keyboard("{Escape}");
    expect(screen.getByRole("dialog", { name: t("restore.wizard.title") })).toBeInTheDocument();

    await user.click(button(wizard, t("restore.wizard.download.cancel")));

    expect(drive.cancel).toHaveBeenCalledTimes(1);
    expect(await within(wizard).findByRole("status")).toHaveTextContent(t("restore.wizard.error.downloadCancelled"));
    expect(await findStep(wizard, t("restore.wizard.list.title"))).toHaveFocus();
    const radios = within(wizard).getAllByRole("radio");
    expect(radios[1]).toBeChecked();
    expect(button(wizard, t("restore.wizard.list.restore"))).toBeEnabled();
    expect(imports).toEqual([]);
  });

  it.each([
    ["network", "restore.wizard.error.network"],
    ["quota", "restore.wizard.error.quota"],
    ["notFound", "restore.wizard.error.notFound"],
    ["server", "restore.wizard.error.server"],
  ] as const)("says what went wrong when the download fails with %s, and keeps the list", async (reason, key) => {
    const { files } = await disk();
    const drive = fakeDrive(CONNECTED);
    drive.download.mockRejectedValue(driveError(reason));
    const { user } = installation(files, { drive });
    const wizard = await openWizard(user);
    await user.click(await within(wizard).findByRole("button", { name: t("restore.wizard.drive.show") }));
    await findStep(wizard, t("restore.wizard.list.title"));
    await user.click(button(wizard, t("restore.wizard.list.restore")));

    expect(await within(wizard).findByRole("alert")).toHaveTextContent(t(key));
    expect(within(wizard).getByRole("heading", { name: t("restore.wizard.list.title") })).toBeInTheDocument();
    expect(within(wizard).getAllByRole("radio")).toHaveLength(2);
  });

  it("goes back to Connect when Google refuses the connection while listing or downloading", async () => {
    const { files } = await disk();
    const drive = fakeDrive(CONNECTED);
    drive.listBackups.mockRejectedValueOnce(driveError("reconnect"));
    drive.download.mockRejectedValueOnce(driveError("notConnected"));
    const { user } = installation(files, { drive });
    const wizard = await openWizard(user);

    // While listing: still on the source, with the reason.
    await user.click(await within(wizard).findByRole("button", { name: t("restore.wizard.drive.show") }));
    expect(await within(wizard).findByRole("alert")).toHaveTextContent(t("restore.wizard.error.reconnect"));
    expect(within(wizard).getByRole("heading", { name: t("restore.wizard.source.title") })).toBeInTheDocument();

    // The next listing works; the download then finds the connection gone and sends the person back to the source.
    await user.click(button(wizard, t("restore.wizard.drive.show")));
    await findStep(wizard, t("restore.wizard.list.title"));
    await user.click(button(wizard, t("restore.wizard.list.restore")));
    expect(await within(wizard).findByRole("alert")).toHaveTextContent(t("restore.wizard.error.notConnected"));
    expect(await findStep(wizard, t("restore.wizard.source.title"))).toBeInTheDocument();
  });

  it("shows the core's own words when a Drive command fails without a known reason", async () => {
    const { files } = await disk();
    const drive = fakeDrive(CONNECTED);
    drive.listBackups.mockRejectedValue(new CoreError({ code: "Internal", message: "something odd" }));
    const { user } = installation(files, { drive });
    const wizard = await openWizard(user);
    await user.click(await within(wizard).findByRole("button", { name: t("restore.wizard.drive.show") }));

    expect(await within(wizard).findByRole("alert")).toHaveTextContent(
      t("restore.wizard.error.generic", { message: "something odd" }),
    );
  });
});

describe("restore wizard: the three sources together", () => {
  it("offers the file, the folder and Drive, in that order, and moves focus to the heading of each step", async () => {
    const { files } = await disk();
    const drive = fakeDrive(DISCONNECTED);
    const list = vi.fn<FolderList>(() => Promise.resolve({ folder: FOLDER, backups: [FOLDER_NEWER] }));
    const { user } = installation(files, { drive, backupFolderList: list });
    const wizard = await openWizard(user);
    await within(wizard).findByRole("button", { name: t("restore.wizard.drive.connect") });

    const names = within(wizard)
      .getAllByRole("button")
      .map((control) => control.textContent);
    expect(names).toEqual([
      t("restore.wizard.source.file"),
      t("restore.wizard.source.folder"),
      t("restore.wizard.drive.connect"),
      t("dialog.cancel"),
    ]);
    expect(within(wizard).getByRole("heading", { name: t("restore.wizard.source.title") })).toHaveFocus();

    await user.click(button(wizard, t("restore.wizard.source.folder")));
    expect(await findStep(wizard, t("restore.wizard.list.title"))).toHaveFocus();
    await user.click(button(wizard, t("onboarding.back")));
    expect(await findStep(wizard, t("restore.wizard.source.title"))).toHaveFocus();
  });
});
