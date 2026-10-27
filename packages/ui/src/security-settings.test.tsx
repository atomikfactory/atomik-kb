import { CoreClient, CoreError, MemoryTransport, kitWordsFromEntropy } from "@atomik-kb/core-client";
import type { BackupResult, DeviceEntry, MemoryTransportOptions, Transport } from "@atomik-kb/core-client";
import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { formatBytes, formatTimestamp } from "./format";
import { t } from "./i18n";
import type { MessageKey } from "./i18n";
import { AUTO_LOCK_KEY } from "./store-security";
import { NOW, findMainNavigation, openScreen, recordingTransport, renderApp, renderWith } from "./test/render";
import type { User } from "./test/render";
import { PASSPHRASE, answerCheck, completeKitFlow, shownWords } from "./test/security";
import { sidebarButton } from "./test/sidebar";

const KIT_KEY = new Uint8Array(32).fill(3);
const KIT = { kitCreatedAt: NOW, kitKey: KIT_KEY };
const LOCK_ON = { ...KIT, appLockPassphrase: PASSPHRASE };
const LATER = NOW + 86_400_000;

afterEach(() => {
  vi.restoreAllMocks();
});

async function openSecurity(options: MemoryTransportOptions = {}) {
  const app = renderApp({ workspaceName: "Research", ...options });
  await openScreen(app.user, "nav.settings");
  await screen.findByRole("heading", { name: t("settings.title") });
  return app;
}

/** Settings over any transport. */
async function openSecurityWith(transport: Transport) {
  const app = renderWith(transport);
  await openScreen(app.user, "nav.settings");
  await screen.findByRole("heading", { name: t("settings.title") });
  return app;
}

/** A subsection of Settings → Security, by its heading. */
function part(name: MessageKey): HTMLElement {
  return screen.getByRole("region", { name: t(name) });
}

/** The `<dd>` next to the `<dt>` with this label, inside `container`. */
function fact(container: HTMLElement, label: MessageKey): HTMLElement | null {
  const term = within(container).getByText(t(label), { selector: "dt" });
  return term.nextElementSibling as HTMLElement | null;
}

/** A transport around `memory` with some of its parts replaced. */
function around(memory: MemoryTransport, overrides: Partial<Transport> = {}): Transport {
  return {
    dispatch: (request) => memory.dispatch(request),
    subscribe: (onEvent) => memory.subscribe(onEvent),
    coreState: () => memory.coreState(),
    docs: memory.docs,
    lock: () => memory.lock(),
    unlock: (passphrase) => memory.unlock(passphrase),
    backupExport: (path) => memory.backupExport(path),
    backupImport: (path, kit) => memory.backupImport(path, kit),
    ...overrides,
  };
}

async function verifies(memory: MemoryTransport, words: readonly string[]): Promise<boolean> {
  return new CoreClient(memory).recoveryKitVerify(words.join(" ")).then(
    () => true,
    () => false,
  );
}

describe("settings: recovery kit", () => {
  it("offers to create the kit when there is none, with the same flow as the onboarding", async () => {
    const { user, memory } = await openSecurity();
    const kit = part("security.kit.title");
    expect(kit).toHaveTextContent(t("security.kit.missing"));
    // The reminder shows in Settings too.
    expect(screen.getByRole("complementary", { name: t("kitBanner.label") })).toBeInTheDocument();

    await user.click(within(kit).getByRole("button", { name: t("security.kit.create") }));
    await screen.findByRole("dialog", { name: t("kit.title") });
    const words = await completeKitFlow(user);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(await verifies(memory, words)).toBe(true);
    await waitFor(() => {
      expect(fact(part("security.kit.title"), "security.kit.created")?.querySelector("time")).toHaveAttribute(
        "datetime",
        new Date(NOW).toISOString(),
      );
    });
    expect(screen.queryByRole("complementary", { name: t("kitBanner.label") })).not.toBeInTheDocument();
  });

  it("shows when the kit was created and whether it was checked", async () => {
    await openSecurity({ security: KIT });
    const kit = part("security.kit.title");
    expect(fact(kit, "security.kit.created")?.querySelector("time")).toHaveAttribute(
      "datetime",
      new Date(NOW).toISOString(),
    );
    expect(fact(kit, "security.kit.verified")).toHaveTextContent(t("security.kit.notVerified"));
    expect(within(kit).queryByText(t("security.kit.reissued"))).not.toBeInTheDocument();
    expect(screen.queryByRole("complementary", { name: t("kitBanner.label") })).not.toBeInTheDocument();
  });

  it("shows the date of the last check", async () => {
    await openSecurity({ security: { ...KIT, kitVerifiedAt: LATER } });
    const verified = fact(part("security.kit.title"), "security.kit.verified");
    expect(verified?.querySelector("time")).toHaveAttribute("datetime", new Date(LATER).toISOString());
    expect(verified).toHaveTextContent(formatTimestamp(LATER)?.text ?? "");
  });

  it("re-issues the kit after a warning, and shows the new one once", async () => {
    const { user, memory } = await openSecurity({ security: KIT });
    const old = await kitWordsFromEntropy(KIT_KEY);
    await user.click(within(part("security.kit.title")).getByRole("button", { name: t("security.kit.reissue") }));

    const dialog = await screen.findByRole("dialog", { name: t("security.reissue.title") });
    expect(dialog).toHaveAccessibleDescription(t("security.reissue.body"));
    expect(within(dialog).queryByLabelText(t("security.reissue.passphraseLabel"))).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("security.reissue.confirm") }));

    // The same dialog now shows the new kit, with the onboarding's grid; Escape does not close it.
    await within(dialog).findByRole("list", { name: t("kit.wordsLabel") });
    expect(dialog).toHaveAccessibleName(t("security.reissue.newTitle"));
    const words = shownWords();
    expect(words).toHaveLength(24);
    expect(words).not.toEqual(old);
    await user.keyboard("{Escape}");
    expect(screen.getByRole("dialog", { name: t("security.reissue.newTitle") })).toBeInTheDocument();
    expect(await verifies(memory, old)).toBe(false);

    await user.click(within(dialog).getByRole("button", { name: t("kit.saved") }));
    await answerCheck(user, words);
    await user.click(await within(dialog).findByRole("button", { name: t("kit.done") }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await waitFor(() => {
      expect(fact(part("security.kit.title"), "security.kit.reissued")?.querySelector("time")).toHaveAttribute(
        "datetime",
        new Date(NOW).toISOString(),
      );
    });
    expect(fact(part("security.kit.title"), "security.kit.verified")?.querySelector("time")).not.toBeNull();
  });

  it("asks for the app-lock passphrase to re-issue while the lock is on", async () => {
    const { user } = await openSecurity({ security: LOCK_ON });
    await user.click(within(part("security.kit.title")).getByRole("button", { name: t("security.kit.reissue") }));
    const dialog = await screen.findByRole("dialog", { name: t("security.reissue.title") });
    const field = within(dialog).getByLabelText(t("security.reissue.passphraseLabel"));
    expect(field).toHaveAttribute("type", "password");
    expect(field).toHaveAttribute("autocomplete", "current-password");
    expect(field).toHaveFocus();

    await user.click(within(dialog).getByRole("button", { name: t("security.reissue.confirm") }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("security.reissue.error.empty"));
    await user.type(field, "not the passphrase");
    await user.click(within(dialog).getByRole("button", { name: t("security.reissue.confirm") }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("security.reissue.error.wrong"));
    expect(field).toHaveValue("");

    await user.type(field, PASSPHRASE);
    await user.click(within(dialog).getByRole("button", { name: t("security.reissue.confirm") }));
    expect(await within(dialog).findByRole("list", { name: t("kit.wordsLabel") })).toBeInTheDocument();
  });

  it("keeps the kit when the re-issue is cancelled", async () => {
    const { user, memory } = await openSecurity({ security: KIT });
    const reissue = within(part("security.kit.title")).getByRole("button", { name: t("security.kit.reissue") });
    await user.click(reissue);
    const dialog = await screen.findByRole("dialog", { name: t("security.reissue.title") });
    expect(within(dialog).getByRole("button", { name: t("dialog.cancel") })).toHaveFocus();
    await user.click(within(dialog).getByRole("button", { name: t("dialog.cancel") }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(reissue).toHaveFocus();
    expect(await verifies(memory, await kitWordsFromEntropy(KIT_KEY))).toBe(true);
  });
});

describe("settings: app lock", () => {
  async function openLockForm(user: User, button: MessageKey): Promise<HTMLElement> {
    const lock = part("security.lock.title");
    await user.click(within(lock).getByRole("button", { name: t(button) }));
    return lock;
  }

  it("turns the lock on with a passphrase typed twice, at least 8 characters", async () => {
    const { user, memory } = await openSecurity({ security: KIT });
    const lock = part("security.lock.title");
    expect(lock).toHaveTextContent(t("security.lock.off"));
    expect(screen.queryByRole("button", { name: t("sidebar.lockNow") })).not.toBeInTheDocument();
    await openLockForm(user, "security.lock.enable");

    const first = within(lock).getByLabelText(t("security.lock.passphraseLabel"));
    const second = within(lock).getByLabelText(t("security.lock.repeatLabel"));
    for (const field of [first, second]) {
      expect(field).toHaveAttribute("type", "password");
      expect(field).toHaveAttribute("autocomplete", "new-password");
    }
    expect(first).toHaveFocus();
    const submit = within(lock).getByRole("button", { name: t("security.lock.enableSubmit") });

    await user.type(first, "short");
    await user.type(second, "short");
    await user.click(submit);
    expect(await within(lock).findByRole("alert")).toHaveTextContent(t("security.lock.error.short"));
    expect(first).toHaveAttribute("aria-invalid", "true");

    await user.clear(first);
    await user.type(first, PASSPHRASE);
    await user.clear(second);
    await user.type(second, `${PASSPHRASE}!`);
    await user.click(submit);
    expect(await within(lock).findByRole("alert")).toHaveTextContent(t("security.lock.error.mismatch"));
    expect(second).toHaveAttribute("aria-invalid", "true");
    expect((await new CoreClient(memory).securityStatus()).appLock.enabled).toBe(false);

    await user.clear(second);
    await user.type(second, PASSPHRASE);
    await user.click(submit);
    expect(await within(lock).findByText(t("security.lock.done.enabled"))).toBeInTheDocument();
    await waitFor(() => {
      expect(lock).toHaveTextContent(t("security.lock.on"));
    });
    expect((await new CoreClient(memory).securityStatus()).appLock.enabled).toBe(true);
    expect(sidebarButton(t("sidebar.lockNow"))).toBeInTheDocument();
  });

  it("changes the passphrase after checking the current one", async () => {
    const { user } = await openSecurity({ security: LOCK_ON });
    const lock = await openLockForm(user, "security.lock.change");
    const current = within(lock).getByLabelText(t("security.lock.currentLabel"));
    expect(current).toHaveAttribute("autocomplete", "current-password");
    expect(current).toHaveFocus();
    const next = within(lock).getByLabelText(t("security.lock.nextLabel"));
    const repeat = within(lock).getByLabelText(t("security.lock.repeatNextLabel"));
    expect(next).toHaveAttribute("autocomplete", "new-password");
    const submit = within(lock).getByRole("button", { name: t("security.lock.changeSubmit") });

    await user.type(current, "not the passphrase");
    await user.type(next, "a new passphrase");
    await user.type(repeat, "a new passphrase");
    await user.click(submit);
    expect(await within(lock).findByRole("alert")).toHaveTextContent(t("security.lock.error.wrong"));
    expect(current).toHaveValue("");
    expect(current).toHaveAttribute("aria-invalid", "true");

    await user.type(current, PASSPHRASE);
    await user.click(submit);
    expect(await within(lock).findByText(t("security.lock.done.changed"))).toBeInTheDocument();

    // The new passphrase unlocks; the old one does not.
    await user.click(sidebarButton(t("sidebar.lockNow")));
    const field = await screen.findByLabelText(t("lock.passphraseLabel"));
    await user.type(field, PASSPHRASE);
    await user.click(screen.getByRole("button", { name: t("lock.submit") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("lock.error.wrong"));
    await user.type(screen.getByLabelText(t("lock.passphraseLabel")), "a new passphrase");
    await user.click(screen.getByRole("button", { name: t("lock.submit") }));
    await findMainNavigation();
    // Back on the screen the lock came from.
    expect(await screen.findByRole("heading", { name: t("settings.title") })).toBeInTheDocument();
  });

  it("turns the lock off with the passphrase", async () => {
    const { user, memory } = await openSecurity({ security: LOCK_ON, settings: { [AUTO_LOCK_KEY]: 5 } });
    expect(screen.getByRole("combobox", { name: t("security.lock.auto") })).toBeInTheDocument();
    const lock = await openLockForm(user, "security.lock.disable");
    const current = within(lock).getByLabelText(t("security.lock.currentLabel"));
    const submit = within(lock).getByRole("button", { name: t("security.lock.disableSubmit") });

    await user.click(submit);
    expect(await within(lock).findByRole("alert")).toHaveTextContent(t("security.lock.error.empty"));
    await user.type(current, "not the passphrase");
    await user.click(submit);
    expect(await within(lock).findByRole("alert")).toHaveTextContent(t("security.lock.error.wrong"));

    await user.type(current, PASSPHRASE);
    await user.click(submit);
    expect(await within(lock).findByText(t("security.lock.done.disabled"))).toBeInTheDocument();
    await waitFor(() => {
      expect(lock).toHaveTextContent(t("security.lock.off"));
    });
    expect((await new CoreClient(memory).securityStatus()).appLock.enabled).toBe(false);
    expect(screen.queryByRole("button", { name: t("sidebar.lockNow") })).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: t("security.lock.auto") })).not.toBeInTheDocument();
  });

  it("closes a form on Cancel without changing anything", async () => {
    const { user, memory } = await openSecurity({ security: LOCK_ON });
    const lock = await openLockForm(user, "security.lock.disable");
    await user.type(within(lock).getByLabelText(t("security.lock.currentLabel")), PASSPHRASE);
    await user.click(within(lock).getByRole("button", { name: t("dialog.cancel") }));
    expect(within(lock).queryByLabelText(t("security.lock.currentLabel"))).not.toBeInTheDocument();
    expect((await new CoreClient(memory).securityStatus()).appLock.enabled).toBe(true);
  });

  it("locks from Settings", async () => {
    const { user, memory } = await openSecurity({ security: LOCK_ON });
    await user.click(within(part("security.lock.title")).getByRole("button", { name: t("security.lock.lockNow") }));
    await screen.findByLabelText(t("lock.passphraseLabel"));
    await expect(memory.coreState()).resolves.toBe("locked");
  });

  it("offers the auto-lock delays while the lock is on and stores the choice", async () => {
    const { user, memory } = await openSecurity({ security: LOCK_ON });
    const select = screen.getByRole("combobox", { name: t("security.lock.auto") });
    expect(within(select).getAllByRole("option").map((option) => option.textContent)).toEqual([
      t("security.lock.auto.0"),
      t("security.lock.auto.1"),
      t("security.lock.auto.5"),
      t("security.lock.auto.15"),
      t("security.lock.auto.60"),
    ]);
    expect(select).toHaveValue("0");
    await user.selectOptions(select, t("security.lock.auto.5"));
    await waitFor(async () => {
      await expect(new CoreClient(memory).settingsGet(AUTO_LOCK_KEY)).resolves.toBe(5);
    });
    expect(select).toHaveValue("5");
  });

  it("follows a delay set elsewhere, and treats unknown values as never", async () => {
    const { memory } = await openSecurity({ security: LOCK_ON, settings: { [AUTO_LOCK_KEY]: 7 } });
    const select = screen.getByRole("combobox", { name: t("security.lock.auto") });
    expect(select).toHaveValue("0");
    await memory.dispatch({ type: "SettingsSet", data: { key: AUTO_LOCK_KEY, value: 60 } });
    await waitFor(() => {
      expect(select).toHaveValue("60");
    });
  });

  it("says when the delay could not be saved", async () => {
    const { user, memory } = await openSecurity({ security: LOCK_ON });
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    await user.selectOptions(screen.getByRole("combobox", { name: t("security.lock.auto") }), t("security.lock.auto.1"));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("security.lock.error.auto", { message: "database error: disk I/O error" }),
    );
  });

  it("says so where the shell cannot lock", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", security: KIT });
    await openSecurityWith(around(memory, { lock: undefined, unlock: undefined }));
    const lock = part("security.lock.title");
    expect(lock).toHaveTextContent(t("security.lock.unavailable"));
    expect(within(lock).queryByRole("button")).not.toBeInTheDocument();
  });
});

describe("settings: devices", () => {
  it("lists the devices of the account with this device marked", async () => {
    await openSecurity({ security: KIT, deviceName: "Desk PC" });
    const devices = within(part("security.devices.title")).getByRole("list");
    const [item] = within(devices).getAllByRole("listitem");
    expect(item).toHaveTextContent("Desk PC");
    expect(item).toHaveTextContent(t("security.devices.thisDevice"));
    expect(item).toHaveTextContent(t("security.devices.added", { date: formatTimestamp(NOW)?.text ?? "" }));
    expect(item).not.toHaveTextContent(t("security.devices.viaKit"));
  });

  it("is empty before the kit exists", async () => {
    await openSecurity();
    expect(part("security.devices.title")).toHaveTextContent(t("security.devices.empty"));
  });

  it("shows devices added with the kit and revoked ones", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", security: KIT });
    const others: DeviceEntry[] = [
      { deviceId: "d2", name: "Old phone", addedAt: NOW, addedViaKit: false, revokedAt: LATER, isThisDevice: false, signingKey: "aa" },
      { deviceId: "d3", name: "Restored laptop", addedAt: LATER, addedViaKit: true, isThisDevice: false, signingKey: "bb" },
    ];
    await openSecurityWith(
      around(memory, {
        dispatch: async (request) => {
          const response = await memory.dispatch(request);
          return response.type === "Devices" ? { type: "Devices", data: [...response.data, ...others] } : response;
        },
      }),
    );
    const items = within(within(part("security.devices.title")).getByRole("list")).getAllByRole("listitem");
    expect(items).toHaveLength(3);
    expect(items[1]).toHaveTextContent(t("security.devices.revoked", { date: formatTimestamp(LATER)?.text ?? "" }));
    expect(items[1]).not.toHaveTextContent(t("security.devices.thisDevice"));
    expect(items[2]).toHaveTextContent(t("security.devices.viaKit"));
  });

  it("follows a rename of this device", async () => {
    const { memory } = await openSecurity({ security: KIT, deviceName: "Desk PC" });
    await memory.dispatch({ type: "DeviceRename", data: { name: "Work laptop" } });
    await waitFor(() => {
      expect(part("security.devices.title")).toHaveTextContent("Work laptop");
    });
  });
});

describe("settings: backup", () => {
  it("exports a backup and says where it went and what it holds", async () => {
    const { user, memory } = await openSecurity({
      security: KIT,
      spaces: [{ name: "Personal", pages: [{ title: "Welcome" }] }],
    });
    const exported = vi.spyOn(memory, "backupExport");
    const backup = part("security.backup.title");
    expect(backup).toHaveTextContent(t("security.backup.intro"));
    await user.click(within(backup).getByRole("button", { name: t("security.backup.export") }));

    expect(await within(backup).findByRole("status")).toHaveTextContent(t("security.backup.done"));
    const result = (await exported.mock.results[0]?.value) as BackupResult;
    // The file name the save dialog proposes (the mirror's dialog picks its directory).
    expect(result.path).toMatch(/atomik-kb-2026-10-03\.atomikkb$/);
    expect(memory.files.has(result.path)).toBe(true);
    expect(fact(backup, "security.backup.path")).toHaveTextContent(result.path);
    expect(fact(backup, "security.backup.documents")).toHaveTextContent(String(result.summary.documents));
    expect(fact(backup, "security.backup.settings")).toHaveTextContent(String(result.summary.settings));
    expect(fact(backup, "security.backup.size")).toHaveTextContent(formatBytes(result.summary.bytes));
    expect(fact(backup, "security.backup.created")?.querySelector("time")).toHaveAttribute(
      "datetime",
      new Date(NOW).toISOString(),
    );
    expect(backup).toHaveTextContent(t("security.backup.restoreNote"));
  });

  it("says so when no file was chosen", async () => {
    const { user } = await openSecurity({ security: KIT, cancelDialogs: true });
    const backup = part("security.backup.title");
    await user.click(within(backup).getByRole("button", { name: t("security.backup.export") }));
    expect(await within(backup).findByRole("status")).toHaveTextContent(t("security.backup.cancelled"));
  });

  it("needs the recovery kit first", async () => {
    await openSecurity();
    const backup = part("security.backup.title");
    const button = within(backup).getByRole("button", { name: t("security.backup.export") });
    expect(button).toBeDisabled();
    expect(button).toHaveAccessibleDescription(t("security.backup.needsKit"));
  });

  it("explains the core's refusal to export without a kit", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", security: KIT });
    const { user } = await openSecurityWith(
      around(memory, {
        backupExport: () =>
          Promise.reject(
            new CoreError({ code: "InvalidInput", message: "create a recovery kit first", details: { field: "kit" } }),
          ),
      }),
    );
    const backup = part("security.backup.title");
    await user.click(within(backup).getByRole("button", { name: t("security.backup.export") }));
    expect(await within(backup).findByRole("alert")).toHaveTextContent(t("security.backup.needsKit"));
  });

  it("shows other failures", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", security: KIT });
    const { user } = await openSecurityWith(
      around(memory, {
        backupExport: () => Promise.reject(new CoreError({ code: "Storage", message: "disk full" })),
      }),
    );
    const backup = part("security.backup.title");
    await user.click(within(backup).getByRole("button", { name: t("security.backup.export") }));
    expect(await within(backup).findByRole("alert")).toHaveTextContent(
      t("security.backup.error", { message: "disk full" }),
    );
  });

  it("says 'Exporting...' while the shell writes the file", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", security: KIT });
    const recorder = recordingTransport(memory);
    const { user } = await openSecurityWith(recorder.transport);
    const backup = part("security.backup.title");
    const release = recorder.hold();
    await user.click(within(backup).getByRole("button", { name: t("security.backup.export") }));
    expect(await within(backup).findByRole("button", { name: t("security.backup.exporting") })).toBeDisabled();
    release();
    expect(await within(backup).findByRole("status")).toHaveTextContent(t("security.backup.done"));
  });

  it("says so where the shell cannot write backup files", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", security: KIT });
    await openSecurityWith(around(memory, { backupExport: undefined, backupImport: undefined }));
    const button = within(part("security.backup.title")).getByRole("button", { name: t("security.backup.export") });
    expect(button).toBeDisabled();
    expect(button).toHaveAccessibleDescription(t("security.backup.unavailable"));
  });
});

describe("settings: security status", () => {
  it("follows SecurityChanged: a kit created elsewhere replaces the reminder", async () => {
    const { memory } = await openSecurity({ deviceName: "Desk PC" });
    expect(screen.getByRole("complementary", { name: t("kitBanner.label") })).toBeInTheDocument();
    await memory.dispatch({ type: "RecoveryKitCreate" });
    await waitFor(() => {
      expect(screen.queryByRole("complementary", { name: t("kitBanner.label") })).not.toBeInTheDocument();
    });
    expect(fact(part("security.kit.title"), "security.kit.created")).not.toBeNull();
    expect(part("security.devices.title")).toHaveTextContent("Desk PC");
    expect(within(part("security.backup.title")).getByRole("button", { name: t("security.backup.export") })).toBeEnabled();
  });

  it("says when the status could not be read, and reads it again on request", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", security: KIT });
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    let failures = 1;
    const { user } = await openSecurityWith(
      around(memory, {
        dispatch: (request) => {
          if (request.type === "SecurityStatus" && failures > 0) {
            failures -= 1;
            return Promise.reject(new CoreError({ code: "Storage", message: "disk I/O error" }));
          }
          return memory.dispatch(request);
        },
      }),
    );
    expect(screen.getByText(t("security.unavailable"))).toBeInTheDocument();
    expect(logged).toHaveBeenCalled();
    // Nothing to remind of while the status is unknown.
    expect(screen.queryByRole("complementary", { name: t("kitBanner.label") })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: t("security.retry") }));
    expect(await screen.findByRole("region", { name: t("security.kit.title") })).toBeInTheDocument();
  });
});
