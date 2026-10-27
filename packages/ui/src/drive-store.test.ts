// The Google Drive target in the app store (ADR-0059, store-drive.ts): the capability, reading the status (and not letting an
// old answer win), connecting with its waiting state and its notices by reason, cancelling, disconnecting, the own client, the
// refresh on `SettingChanged`, nothing kept once the app is locked, and the words the shell's consent dialog and sign-in page get.

import { CoreClient, CoreError, MemoryTransport } from "@atomik-kb/core-client";
import type { DriveStatus } from "@atomik-kb/core-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { loadLanguage, resetLanguage } from "./i18n";
import en from "./i18n/en.json";
import es from "./i18n/es.json";
import fr from "./i18n/fr.json";
import { createAppStore } from "./store";
import { BACKUP_DRIVE_KEY, EMPTY_DRIVE, driveNoticeOf } from "./store-drive";
import { dialogLabels } from "./store-shell";
import { connectedStatus, driveFailure, driveStatus, fakeDrive } from "./test/drive";
import type { FakeDrive } from "./test/drive";
import { interopTransport } from "./test/interop";
import type { InteropOptions } from "./test/interop";

afterEach(() => {
  resetLanguage();
  vi.restoreAllMocks();
});

async function setup(drive: FakeDrive | null, memoryOptions: ConstructorParameters<typeof MemoryTransport>[0] = {}) {
  const memory = new MemoryTransport({ workspaceName: "Home", ...memoryOptions });
  const options: InteropOptions = { methods: drive === null ? {} : { drive: drive.api } };
  const wrapped = interopTransport(memory, options);
  const store = createAppStore(new CoreClient(wrapped.transport), { corePollMs: 5 });
  const stop = store.getState().start();
  await vi.waitFor(() => {
    expect(store.getState().phase).toBe("ready");
  });
  return { memory, store, stop, ...wrapped };
}

/** The `SettingChanged` event the core emits when the shell writes the Drive connection. */
const driveSettingChanged = { type: "SettingChanged", data: { key: BACKUP_DRIVE_KEY } } as const;

describe("the capability", () => {
  it("follows the transport's Drive", async () => {
    const without = await setup(null);
    expect(without.store.getState().interopCapabilities.drive).toBe(false);
    // Nothing happens without it, and nothing rejects.
    await without.store.getState().loadDriveStatus();
    await expect(without.store.getState().connectDrive()).resolves.toBe(false);
    expect(without.store.getState().driveStatus).toBeNull();
    without.stop();

    const withDrive = await setup(fakeDrive());
    expect(withDrive.store.getState().interopCapabilities.drive).toBe(true);
    withDrive.stop();
  });
});

describe("the status", () => {
  it("is read on request and kept", async () => {
    const drive = fakeDrive({ client: "own", ownClientId: "my-client" });
    const { store, stop } = await setup(drive);
    expect(store.getState().driveStatus).toBeNull();
    await store.getState().loadDriveStatus();
    expect(store.getState().driveStatus).toEqual(driveStatus({ client: "own", ownClientId: "my-client" }));
    expect(drive.status).toHaveBeenCalledTimes(1);
    stop();
  });

  it("is read again when the shell writes app.backup.drive, once somebody has read it", async () => {
    const drive = fakeDrive();
    const { store, emit, stop } = await setup(drive);
    // Nobody has read it: there is no slice to refresh.
    emit(driveSettingChanged);
    expect(drive.status).not.toHaveBeenCalled();

    await store.getState().loadDriveStatus();
    const last = { at: 5, ok: true, file: "atomik-kb-auto-2026-10-19-0930.atomikkb" };
    drive.change(connectedStatus({ last }));
    emit({ type: "SettingChanged", data: { key: "app.backup.folder" } });
    expect(drive.status).toHaveBeenCalledTimes(1);
    emit(driveSettingChanged);
    await vi.waitFor(() => {
      expect(store.getState().driveStatus).toEqual(connectedStatus({ last }));
    });
    stop();
  });

  it("is not overwritten by a read that left before a newer answer", async () => {
    const drive = fakeDrive();
    const { store, stop } = await setup(drive);
    let answerOld: (status: DriveStatus) => void = () => undefined;
    drive.status.mockImplementationOnce(
      () =>
        new Promise<DriveStatus>((resolve) => {
          answerOld = resolve;
        }),
    );
    const slow = store.getState().loadDriveStatus();
    // A sign-in ends while the slow read is still out.
    await store.getState().connectDrive();
    expect(store.getState().driveStatus?.connected).toBe(true);
    answerOld(driveStatus());
    await slow;
    expect(store.getState().driveStatus?.connected).toBe(true);
    stop();
  });

  it("says why it could not be read, and logs the code only", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const drive = fakeDrive();
    drive.status.mockRejectedValue(driveFailure("network", "secret detail"));
    const { store, stop } = await setup(drive);
    await store.getState().loadDriveStatus();
    expect(store.getState().driveStatus).toBeNull();
    expect(store.getState().driveNotice).toEqual({ kind: "error", reason: "network", message: "secret detail" });
    expect(logged).toHaveBeenCalledWith("could not read the Google Drive status", "InvalidInput");
    stop();
  });
});

describe("connecting", () => {
  it("is a waiting state until the shell answers, then the connected status", async () => {
    const drive = fakeDrive();
    const held = drive.holdConnect();
    const { store, stop } = await setup(drive);
    const connecting = store.getState().connectDrive();
    expect(store.getState().driveConnecting).toBe(true);
    expect(store.getState().driveNotice).toBeNull();
    // One sign-in at a time: another press does nothing.
    await expect(store.getState().connectDrive()).resolves.toBe(false);
    expect(drive.connect).toHaveBeenCalledTimes(1);

    held.finish();
    await expect(connecting).resolves.toBe(true);
    expect(store.getState().driveConnecting).toBe(false);
    expect(store.getState().driveStatus).toEqual(connectedStatus());
    stop();
  });

  it("ends in an information notice when the person declined the consent or cancelled", async () => {
    for (const reason of ["consentDeclined", "cancelled"] as const) {
      const drive = fakeDrive();
      drive.connect.mockRejectedValue(driveFailure(reason));
      const { store, stop } = await setup(drive);
      await expect(store.getState().connectDrive()).resolves.toBe(false);
      expect(store.getState().driveConnecting).toBe(false);
      expect(store.getState().driveNotice).toMatchObject({ kind: "info", reason });
      stop();
    }
  });

  it("keeps the reason of every other failure, and the message of one without a reason", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    for (const reason of ["noClient", "browserTimeout", "denied", "reconnect", "quota", "network", "server"] as const) {
      const drive = fakeDrive();
      drive.connect.mockRejectedValue(driveFailure(reason));
      const { store, stop } = await setup(drive);
      await store.getState().connectDrive();
      expect(store.getState().driveNotice, reason).toMatchObject({ kind: "error", reason });
      stop();
    }
    expect(driveNoticeOf(new CoreError({ code: "Internal", message: "odd" }))).toEqual({ kind: "error", reason: null, message: "odd" });
    expect(driveNoticeOf(new Error("plain"))).toMatchObject({ kind: "error", reason: null });
  });

  it("drops the old notice when it starts again, and clearDriveNotice drops it too", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const drive = fakeDrive();
    drive.connect.mockRejectedValueOnce(driveFailure("denied"));
    const { store, stop } = await setup(drive);
    await store.getState().connectDrive();
    expect(store.getState().driveNotice?.reason).toBe("denied");
    store.getState().clearDriveNotice();
    expect(store.getState().driveNotice).toBeNull();
    drive.connect.mockRejectedValueOnce(driveFailure("denied"));
    await store.getState().connectDrive();
    expect(store.getState().driveNotice?.reason).toBe("denied");
    await expect(store.getState().connectDrive()).resolves.toBe(true);
    expect(store.getState().driveNotice).toBeNull();
    stop();
  });

  it("asks the shell to cancel only while it waits, and ends with the notice 'cancelled'", async () => {
    const drive = fakeDrive();
    const held = drive.holdConnect();
    const { store, stop } = await setup(drive);
    await store.getState().cancelDriveConnect();
    expect(drive.cancel).not.toHaveBeenCalled();

    const connecting = store.getState().connectDrive();
    await store.getState().cancelDriveConnect();
    expect(drive.cancel).toHaveBeenCalledTimes(1);
    held.fail(driveFailure("cancelled"));
    await expect(connecting).resolves.toBe(false);
    expect(store.getState().driveConnecting).toBe(false);
    expect(store.getState().driveNotice).toMatchObject({ kind: "info", reason: "cancelled" });
    stop();
  });

  it("logs a failed cancel by its code", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const drive = fakeDrive();
    drive.holdConnect();
    drive.cancel.mockRejectedValue(new CoreError({ code: "Internal", message: "nope" }));
    const { store, stop } = await setup(drive);
    void store.getState().connectDrive();
    await store.getState().cancelDriveConnect();
    expect(logged).toHaveBeenCalledWith("a Google Drive sign-in could not be cancelled", "Internal");
    stop();
  });
});

describe("disconnecting and the own client", () => {
  it("disconnects, and answers whether it did", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const drive = fakeDrive(connectedStatus());
    const { store, stop } = await setup(drive);
    await store.getState().loadDriveStatus();
    await expect(store.getState().disconnectDrive()).resolves.toBe(true);
    expect(store.getState().driveStatus).toEqual(driveStatus());
    expect(store.getState().driveBusy).toBe(false);

    drive.disconnect.mockRejectedValueOnce(driveFailure("network"));
    await expect(store.getState().disconnectDrive()).resolves.toBe(false);
    expect(store.getState().driveNotice).toMatchObject({ kind: "error", reason: "network" });
    expect(store.getState().driveBusy).toBe(false);
    stop();
  });

  it("sets one's own client and goes back to the built-in one", async () => {
    const drive = fakeDrive(connectedStatus());
    const { store, stop } = await setup(drive);
    await expect(store.getState().setDriveClient({ clientId: "my-client", clientSecret: "shh" })).resolves.toBe(true);
    expect(drive.setClient).toHaveBeenCalledWith({ clientId: "my-client", clientSecret: "shh" });
    expect(store.getState().driveStatus).toEqual(driveStatus({ client: "own", ownClientId: "my-client" }));
    // The secret is never kept in the store.
    expect(JSON.stringify(store.getState().driveStatus)).not.toContain("shh");

    await expect(store.getState().setDriveClient(null)).resolves.toBe(true);
    expect(store.getState().driveStatus).toEqual(driveStatus());
    stop();
  });

  it("is busy while it runs, and refuses a second action meanwhile", async () => {
    const drive = fakeDrive(connectedStatus());
    let finish: (status: DriveStatus) => void = () => undefined;
    drive.disconnect.mockImplementationOnce(
      () =>
        new Promise<DriveStatus>((resolve) => {
          finish = resolve;
        }),
    );
    const { store, stop } = await setup(drive);
    const first = store.getState().disconnectDrive();
    expect(store.getState().driveBusy).toBe(true);
    await expect(store.getState().connectDrive()).resolves.toBe(false);
    await expect(store.getState().setDriveClient(null)).resolves.toBe(false);
    expect(drive.connect).not.toHaveBeenCalled();
    finish(driveStatus());
    await expect(first).resolves.toBe(true);
    expect(store.getState().driveBusy).toBe(false);
    stop();
  });
});

describe("the lock", () => {
  const LOCKABLE = { security: { kitCreatedAt: 1, kitKey: new Uint8Array(32).fill(3), appLockPassphrase: "correct horse battery" } };

  it("forgets the account, cancels a sign-in that waits and drops what it would still deliver", async () => {
    const drive = fakeDrive();
    const held = drive.holdConnect();
    const { store, stop } = await setup(drive, LOCKABLE);
    await store.getState().loadDriveStatus();
    const connecting = store.getState().connectDrive();
    expect(store.getState().driveConnecting).toBe(true);

    await store.getState().lock();
    expect(store.getState().phase).toBe("locked");
    expect(store.getState()).toMatchObject(EMPTY_DRIVE);
    expect(drive.cancel).toHaveBeenCalledTimes(1);

    // The sign-in ends after the lock (the shell answered before it saw the cancel): nobody is shown it.
    held.finish();
    await expect(connecting).resolves.toBe(false);
    expect(store.getState().driveStatus).toBeNull();
    expect(store.getState().driveConnecting).toBe(false);
    stop();
  });
});

describe("the words of the shell's Drive dialog and sign-in page", () => {
  const files = { en, fr, es } as const;
  const KEYS = {
    dialogDriveTitle: "shellDialog.driveTitle",
    dialogDriveMessage: "shellDialog.driveMessage",
    dialogDriveAccept: "shellDialog.driveAccept",
    dialogDriveCancel: "shellDialog.driveCancel",
    dialogDriveDone: "shellDialog.driveDone",
  } as const;

  it("are sent from the shellDialog.drive* keys, in the person's language", async () => {
    const english = dialogLabels();
    for (const [label, key] of Object.entries(KEYS)) {
      expect(english[label as keyof typeof KEYS], label).toBe(en[key]);
    }
    await loadLanguage("fr");
    expect(dialogLabels().dialogDriveTitle).toBe(fr["shellDialog.driveTitle"]);
    expect(dialogLabels().dialogDriveAccept).toBe("Connecter");
    await loadLanguage("es");
    expect(dialogLabels().dialogDriveMessage).toBe(es["shellDialog.driveMessage"]);
    expect(dialogLabels().dialogDriveDone).toBe(es["shellDialog.driveDone"]);
  });

  it.each(["en", "fr", "es"] as const)("fit what the shell accepts in %s: a label up to 64 characters, a text up to 400, no placeholder", (language) => {
    for (const [label, key] of Object.entries(KEYS)) {
      const text = files[language][key];
      const isText = label === "dialogDriveMessage" || label === "dialogDriveDone";
      expect(text.length, label).toBeGreaterThanOrEqual(1);
      expect(text.length, label).toBeLessThanOrEqual(isText ? 400 : 64);
      expect(text.trim(), label).toBe(text);
      // Nothing in them is filled by the shell.
      expect(text, label).not.toMatch(/[{}]/);
      // eslint-disable-next-line no-control-regex -- the shell refuses control characters
      expect(/[ -]/.test(text), label).toBe(false);
    }
  });

  it("say what leaves the computer and what Google can see", () => {
    const message = en["shellDialog.driveMessage"];
    expect(message).toContain("encrypted backups");
    expect(message).toContain("Google account");
    expect(message).toContain("names, sizes and times");
    expect(message).toContain("cannot read");
    expect(message).toContain("stay in Drive");
    expect(en["shellDialog.driveDone"]).toBe("You can close this tab and go back to atomik-kb.");
  });
});
