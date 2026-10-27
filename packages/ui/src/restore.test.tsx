import { CoreClient, CoreError, MemoryTransport, kitWordsFromEntropy } from "@atomik-kb/core-client";
import type { BackupResult, MemoryTransportOptions, Transport } from "@atomik-kb/core-client";
import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t } from "./i18n";
import { NOW, findMainNavigation, openScreen, recordingTransport, renderWith } from "./test/render";
import type { User } from "./test/render";

const BACKUP = "C:\\Users\\you\\Documents\\atomik-kb-2026-10-03.atomikkb";

/** A workspace with a kit, backed up to `BACKUP` on a fake disk that a fresh installation can share. */
async function backupOfResearch() {
  const kitKey = new Uint8Array(32).fill(7);
  const source = new MemoryTransport({
    now: () => NOW,
    workspaceName: "Research",
    spaces: [{ name: "Personal", pages: [{ title: "Welcome" }, { title: "Journal" }] }],
    settings: { "ui.theme": "dark" },
    security: { kitCreatedAt: NOW, kitKey },
  });
  await source.backupExport(BACKUP);
  const accountId = (await new CoreClient(source).securityStatus()).accountId;
  return { files: source.files, words: await kitWordsFromEntropy(kitKey), accountId };
}

/** A fresh installation (no workspace) whose disk holds the backup. */
function freshInstallation(files: Map<string, Uint8Array>, options: MemoryTransportOptions = {}) {
  const memory = new MemoryTransport({ now: () => NOW, files, ...options });
  return { memory, ...renderWith(memory) };
}

function restoreButton(): HTMLElement {
  return screen.getByRole("button", { name: t("onboarding.restore.button") });
}

/** "Restore from a backup" opens the wizard (ADR-0059); "A backup file…" is the flow that was the button's own before it. */
async function chooseFile(user: User): Promise<void> {
  await user.click(restoreButton());
  const wizard = await screen.findByRole("dialog", { name: t("restore.wizard.title") });
  await user.click(within(wizard).getByRole("button", { name: t("restore.wizard.source.file") }));
}

async function findKitDialog(): Promise<HTMLElement> {
  return screen.findByRole("dialog", { name: t("restoreKit.title") });
}

async function typeKit(user: User, dialog: HTMLElement, text: string): Promise<void> {
  const field = within(dialog).getByRole("textbox", { name: t("restoreKit.label") });
  await user.clear(field);
  await user.click(field);
  await user.paste(text);
}

describe("onboarding: restore from a backup", () => {
  it("is offered next to creating a workspace", async () => {
    const { files } = await backupOfResearch();
    freshInstallation(files);
    await screen.findByRole("heading", { name: t("onboarding.title") });
    expect(screen.getByText(t("onboarding.restore.intro"))).toBeInTheDocument();
    expect(restoreButton()).toBeEnabled();
  });

  it("is not offered where the shell has no backup files", async () => {
    const memory = new MemoryTransport({ now: () => NOW });
    const web: Transport = {
      dispatch: (request) => memory.dispatch(request),
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs: memory.docs,
    };
    renderWith(web);
    await screen.findByRole("heading", { name: t("onboarding.title") });
    expect(screen.queryByRole("button", { name: t("onboarding.restore.button") })).not.toBeInTheDocument();
  });

  it("says so when no file was chosen, and can be tried again", async () => {
    const { files } = await backupOfResearch();
    const { user, memory } = freshInstallation(files, { cancelDialogs: true });
    await screen.findByRole("heading", { name: t("onboarding.title") });
    await chooseFile(user);
    expect(await screen.findByRole("status")).toHaveTextContent(t("onboarding.restore.cancelled"));
    // The wizard stays open, ready for another try; there is no kit dialog.
    expect(screen.queryByRole("dialog", { name: t("restoreKit.title") })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("restore.wizard.source.file") })).toBeEnabled();
    await expect(new CoreClient(memory).workspaceGet()).resolves.toBeNull();
  });

  it("asks for the kit, refuses a wrong one, and restores with the right one", async () => {
    const { files, words } = await backupOfResearch();
    const { user, memory } = freshInstallation(files);
    await screen.findByRole("heading", { name: t("onboarding.title") });
    await chooseFile(user);

    const dialog = await findKitDialog();
    expect(within(dialog).getByText(t("restoreKit.body"))).toBeInTheDocument();
    const wrong = await kitWordsFromEntropy(new Uint8Array(32).fill(9));
    await typeKit(user, dialog, wrong.join(" "));
    await user.click(within(dialog).getByRole("button", { name: t("restoreKit.submit") }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("restoreKit.error.wrong"));
    await expect(new CoreClient(memory).workspaceGet()).resolves.toBeNull();

    await typeKit(user, dialog, words.join(" "));
    expect(within(dialog).queryByRole("alert")).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("restoreKit.submit") }));

    // The app starts again in the restored workspace, with its pages and settings.
    const nav = await findMainNavigation();
    expect(within(nav).getByText("Research")).toBeInTheDocument();
    expect(screen.getByRole("treeitem", { name: "Welcome" })).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
    // The kit came with the backup: no reminder to create one.
    expect(screen.queryByRole("complementary", { name: t("kitBanner.label") })).not.toBeInTheDocument();
  });

  it("lists this installation as added with the recovery kit after a restore", async () => {
    const { files, words } = await backupOfResearch();
    const { user } = freshInstallation(files, { deviceName: "New laptop" });
    await screen.findByRole("heading", { name: t("onboarding.title") });
    await chooseFile(user);
    const dialog = await findKitDialog();
    await typeKit(user, dialog, words.join(" "));
    await user.click(within(dialog).getByRole("button", { name: t("restoreKit.submit") }));
    await findMainNavigation();

    await openScreen(user, "nav.settings");
    const devices = await screen.findByRole("list", { name: t("security.devices.title") });
    const items = within(devices).getAllByRole("listitem");
    expect(items).toHaveLength(2);
    const me = items.find((item) => within(item).queryByText(t("security.devices.thisDevice")) !== null);
    expect(me).toHaveTextContent("New laptop");
    expect(me).toHaveTextContent(t("security.devices.viaKit"));
  });

  it("helps while the kit is typed: word count, words not in the list, the code", async () => {
    const { files } = await backupOfResearch();
    const { user } = freshInstallation(files);
    await screen.findByRole("heading", { name: t("onboarding.title") });
    await chooseFile(user);
    const dialog = await findKitDialog();
    const field = within(dialog).getByRole("textbox", { name: t("restoreKit.label") });
    expect(field).toHaveAccessibleDescription(t("restoreKit.hint.prefix"));

    await typeKit(user, dialog, "Abandon abil zzzz");
    expect(within(dialog).getByText(new RegExp(t("restoreKit.hint.count", { count: 3 })))).toBeInTheDocument();
    expect(
      within(dialog).getByText(
        t("restoreKit.hint.unknown", { words: t("restoreKit.hint.unknownWord", { position: 3, word: "zzzz" }) }),
      ),
    ).toBeInTheDocument();

    await typeKit(user, dialog, "ABCD-EFGH-2345");
    expect(within(dialog).getByText(t("restoreKit.hint.code"))).toBeInTheDocument();

    await user.clear(field);
    await user.click(within(dialog).getByRole("button", { name: t("restoreKit.submit") }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("restoreKit.error.empty"));
  });

  it("closes the kit dialog on Cancel without restoring anything", async () => {
    const { files } = await backupOfResearch();
    const { user, memory } = freshInstallation(files);
    await screen.findByRole("heading", { name: t("onboarding.title") });
    await chooseFile(user);
    const dialog = await findKitDialog();
    await user.click(within(dialog).getByRole("button", { name: t("dialog.cancel") }));
    expect(screen.queryByRole("dialog", { name: t("restoreKit.title") })).not.toBeInTheDocument();
    // Back in the wizard, at the heading of its step.
    expect(screen.getByRole("dialog", { name: t("restore.wizard.title") })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: t("restore.wizard.source.title") })).toHaveFocus();
    await expect(new CoreClient(memory).workspaceGet()).resolves.toBeNull();
  });

  it("says so when the backup file is damaged", async () => {
    const { files } = await backupOfResearch();
    const damaged = (files.get(BACKUP) ?? new Uint8Array()).slice();
    const middle = Math.floor(damaged.length / 2);
    damaged[middle] = (damaged[middle] ?? 0) ^ 0x01;
    files.set(BACKUP, damaged);
    const { user, memory } = freshInstallation(files);
    await screen.findByRole("heading", { name: t("onboarding.title") });
    await chooseFile(user);
    expect(await screen.findByRole("alert")).toHaveTextContent(t("onboarding.restore.error.damaged"));
    expect(screen.queryByRole("dialog", { name: t("restoreKit.title") })).not.toBeInTheDocument();
    await expect(new CoreClient(memory).workspaceGet()).resolves.toBeNull();
  });

  it("restores without the kit on an installation that still holds the account's key", async () => {
    const { files, accountId } = await backupOfResearch();
    const { user } = freshInstallation(files, { security: { accountId } });
    await screen.findByRole("heading", { name: t("onboarding.title") });
    await chooseFile(user);
    const nav = await findMainNavigation();
    expect(within(nav).getByText("Research")).toBeInTheDocument();
  });

  it("says 'Restoring...' while the core works", async () => {
    const { files, accountId } = await backupOfResearch();
    const memory = new MemoryTransport({ now: () => NOW, files, security: { accountId } });
    const recorder = recordingTransport(memory);
    const { user } = renderWith(recorder.transport);
    await screen.findByRole("heading", { name: t("onboarding.title") });
    const release = recorder.hold();
    await chooseFile(user);
    expect(await screen.findByRole("button", { name: t("onboarding.restore.working") })).toBeDisabled();
    release();
    await findMainNavigation();
  });

  it("restores the file the shell already read when the core names it, instead of asking again", async () => {
    const { files, words } = await backupOfResearch();
    const memory = new MemoryTransport({ now: () => NOW, files });
    const calls: { path: string | undefined; kit: string | undefined }[] = [];
    const shell: Transport = {
      dispatch: (request) => memory.dispatch(request),
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs: memory.docs,
      lock: () => memory.lock(),
      unlock: (passphrase) => memory.unlock(passphrase),
      backupExport: (path) => memory.backupExport(path),
      backupImport: async (path, kit): Promise<BackupResult> => {
        calls.push({ path, kit });
        if (kit === undefined) {
          throw new CoreError({
            code: "InvalidInput",
            message: "the recovery kit is needed to restore this backup",
            details: { field: "kit", path: BACKUP },
          });
        }
        return memory.backupImport(path, kit);
      },
    };
    const { user } = renderWith(shell);
    await screen.findByRole("heading", { name: t("onboarding.title") });
    await chooseFile(user);
    const dialog = await findKitDialog();
    await typeKit(user, dialog, `1. ${words.slice(0, 12).join(" ")}\n13. ${words.slice(12).join(" ")}`);
    await user.click(within(dialog).getByRole("button", { name: t("restoreKit.submit") }));
    await findMainNavigation();
    // Numbering dropped, words normalised; the file the shell named.
    expect(calls).toEqual([
      { path: undefined, kit: undefined },
      { path: BACKUP, kit: words.join(" ") },
    ]);
  });

  it("shows other failures in the dialog and keeps what was typed", async () => {
    const { files, words } = await backupOfResearch();
    const { user, memory } = freshInstallation(files);
    await screen.findByRole("heading", { name: t("onboarding.title") });
    await chooseFile(user);
    const dialog = await findKitDialog();
    await typeKit(user, dialog, words.join(" "));
    // The open dialog is cancelled on the second try (the core did not name the file).
    memory.setCancelDialogs(true);
    await user.click(within(dialog).getByRole("button", { name: t("restoreKit.submit") }));
    expect(await within(dialog).findByRole("status")).toHaveTextContent(t("onboarding.restore.cancelled"));
    expect(within(dialog).getByRole("textbox", { name: t("restoreKit.label") })).toHaveValue(words.join(" "));
    memory.setCancelDialogs(false);
    await user.click(within(dialog).getByRole("button", { name: t("restoreKit.submit") }));
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    await findMainNavigation();
  });
});
