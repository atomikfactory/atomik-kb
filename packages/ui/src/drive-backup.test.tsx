// Settings -> Security -> Backup -> Scheduled backups -> Google Drive (ADR-0059, `DriveBackupTarget`): hidden without the shell's
// Drive; not connected, connecting (with Cancel), connected, asking to reconnect, without a client (and "Advanced" open), the
// own client form, the errors by reason, the last upload, the status kept fresh, and the rule that turns the schedule on:
// a recovery kit and at least one target (the folder or a connected Drive). The shell is `fakeDrive` (test/drive.ts), a
// `DriveApi` whose calls are spies; the memory core answers the rest.

import { DRIVE_ERROR_REASONS } from "@atomik-kb/core-client";
import type { DriveErrorReason, JsonValue, MemoryTransportOptions } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { formatTimestamp } from "./format";
import { t } from "./i18n";
import { scheduleBlocker } from "./screens/ScheduledBackups";
import { BACKUP_FOLDER_KEY } from "./store-interop";
import { BACKUP_DRIVE_KEY } from "./store-drive";
import { DRIVE_ACCOUNT, DRIVE_FOLDER_URL, connectedStatus, driveFailure, fakeDrive } from "./test/drive";
import type { FakeDrive } from "./test/drive";
import { backupResult, renderInterop } from "./test/interop";
import { NOW, openScreen } from "./test/render";

afterEach(() => {
  vi.restoreAllMocks();
});

const KIT: MemoryTransportOptions["security"] = { kitCreatedAt: NOW, kitKey: new Uint8Array(32).fill(3) };
const FOLDER = "C:\\Users\\Ada\\Backups";
const FILE = "atomik-kb-auto-2026-10-19-0930.atomikkb";

type Run = ReturnType<typeof vi.fn<() => Promise<ReturnType<typeof backupResult>>>>;

interface Setup {
  drive?: FakeDrive | null;
  settings?: Record<string, JsonValue>;
  kit?: boolean;
  /** "Back up now" of the shell (default: one that wrote `FILE`). */
  run?: Run;
}

async function openSettings({ drive = fakeDrive(), settings = {}, kit = true, run }: Setup = {}) {
  const backupRunNow = run ?? vi.fn(() => Promise.resolve(backupResult(`C:\\Backups\\${FILE}`)));
  const ctx = await renderInterop(
    { workspaceName: "Research", settings, security: kit ? KIT : undefined, initialRoute: "settings" },
    {
      methods: (memory) => ({
        backupFolderChoose: () => memory.backupFolderChoose(),
        backupRunNow,
        ...(drive === null ? {} : { drive: drive.api }),
      }),
    },
  );
  await screen.findByRole("heading", { name: t("settings.title") });
  return { ...ctx, drive, backupRunNow };
}

function section(): HTMLElement {
  return screen.getByRole("region", { name: t("security.scheduled.title") });
}

/** The Google Drive target, once it has read its status. */
async function target(): Promise<HTMLElement> {
  const group = await screen.findByRole("group", { name: t("security.scheduled.drive.label") });
  await waitFor(() => {
    expect(within(group).queryByText(t("security.scheduled.drive.checking"))).not.toBeInTheDocument();
  });
  return group;
}

function schedule() {
  const part = within(section());
  return {
    enabled: part.getByRole("checkbox", { name: t("security.scheduled.enabled") }),
    now: part.getByRole("button", { name: t("security.scheduled.now") }),
  };
}

const connectLabel = t("security.scheduled.drive.connect");

describe("without the shell's Drive", () => {
  it("shows no Drive target, and the folder-only words", async () => {
    await openSettings({ drive: null });
    expect(screen.getByRole("region", { name: t("security.scheduled.title") })).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: t("security.scheduled.drive.label") })).not.toBeInTheDocument();
    expect(within(section()).getByText(t("security.scheduled.intro"))).toBeInTheDocument();
    expect(within(section()).getByText(t("security.scheduled.needsFolder"))).toBeInTheDocument();
  });
});

describe("not connected", () => {
  it("offers to connect, reads the status when the section opens, and keeps the schedule off", async () => {
    const { drive } = await openSettings();
    const group = await target();
    expect(drive?.status).toHaveBeenCalled();
    expect(within(section()).getByText(t("security.scheduled.drive.intro"))).toBeInTheDocument();
    expect(within(group).getByText(t("security.scheduled.drive.about"))).toBeInTheDocument();
    expect(within(group).getByRole("button", { name: connectLabel })).toBeEnabled();
    // The folder is the other target; none is set yet.
    expect(within(section()).getByText(t("security.scheduled.drive.needsTarget"))).toBeInTheDocument();
    const { enabled, now } = schedule();
    expect(enabled).toBeDisabled();
    expect(now).toBeDisabled();
    expect(enabled).toHaveAccessibleDescription(t("security.scheduled.drive.needsTarget"));
    // Advanced is there, closed: a client exists.
    const advanced = within(group).getByText(t("security.scheduled.drive.advanced")).closest("details");
    expect(advanced).not.toHaveAttribute("open");
  });
});

describe("connecting", () => {
  it("waits for the browser with Cancel, then shows the account and turns the schedule on", async () => {
    const drive = fakeDrive();
    const held = drive.holdConnect();
    const { user } = await openSettings({ drive });
    const group = await target();
    await user.click(within(group).getByRole("button", { name: connectLabel }));

    expect(drive.connect).toHaveBeenCalledTimes(1);
    expect(await within(group).findByRole("status")).toHaveTextContent(t("security.scheduled.drive.connecting"));
    expect(within(group).queryByRole("button", { name: connectLabel })).not.toBeInTheDocument();
    const cancel = within(group).getByRole("button", { name: t("security.scheduled.drive.cancel") });
    expect(cancel).toHaveFocus();
    // Own client and disconnect cannot be changed meanwhile.
    expect(within(group).getByRole("button", { name: t("security.scheduled.drive.saveClient") })).toBeDisabled();

    await act(async () => {
      held.finish();
      await Promise.resolve();
    });
    expect(await within(group).findByText(t("security.scheduled.drive.connectedAs", { account: DRIVE_ACCOUNT }))).toBeInTheDocument();
    expect(within(group).getByRole("button", { name: t("security.scheduled.drive.openFolder") })).toBeEnabled();
    expect(within(group).getByRole("button", { name: t("security.scheduled.drive.disconnect") })).toBeEnabled();
    expect(within(group).queryByRole("button", { name: connectLabel })).not.toBeInTheDocument();
    // A connected Drive is a target: the schedule and "Back up now" are on without a folder.
    const { enabled, now } = schedule();
    expect(enabled).toBeEnabled();
    expect(now).toBeEnabled();
    expect(within(section()).queryByText(t("security.scheduled.drive.needsTarget"))).not.toBeInTheDocument();
  });

  it("cancels: the shell is asked, and the section says so and offers Connect again", async () => {
    const drive = fakeDrive();
    const held = drive.holdConnect();
    const { user } = await openSettings({ drive });
    const group = await target();
    await user.click(within(group).getByRole("button", { name: connectLabel }));
    await user.click(await within(group).findByRole("button", { name: t("security.scheduled.drive.cancel") }));
    expect(drive.cancel).toHaveBeenCalledTimes(1);
    await act(async () => {
      held.fail(driveFailure("cancelled"));
      await Promise.resolve();
    });
    expect(await within(group).findByRole("status")).toHaveTextContent(t("security.scheduled.drive.error.cancelled"));
    expect(within(group).queryByRole("alert")).not.toBeInTheDocument();
    expect(within(group).getByRole("button", { name: connectLabel })).toBeEnabled();
  });

  it("says that the notice was declined without calling it an error", async () => {
    const drive = fakeDrive();
    drive.connect.mockRejectedValue(driveFailure("consentDeclined"));
    const { user } = await openSettings({ drive });
    const group = await target();
    await user.click(within(group).getByRole("button", { name: connectLabel }));
    expect(await within(group).findByRole("status")).toHaveTextContent(t("security.scheduled.drive.error.consentDeclined"));
    expect(within(group).queryByRole("alert")).not.toBeInTheDocument();
  });

  const FAILURES = DRIVE_ERROR_REASONS.filter((reason) => reason !== "cancelled" && reason !== "consentDeclined");

  it.each(FAILURES)("words the failure %s", async (reason: DriveErrorReason) => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const drive = fakeDrive();
    drive.connect.mockRejectedValue(driveFailure(reason));
    const { user } = await openSettings({ drive });
    const group = await target();
    await user.click(within(group).getByRole("button", { name: connectLabel }));
    expect(await within(group).findByRole("alert")).toHaveTextContent(t(`security.scheduled.drive.error.${reason}`));
    // Back to Connect, so that it can be tried again.
    expect(within(group).getByRole("button", { name: connectLabel })).toBeEnabled();
  });

  it("shows the core's message for a failure that has no reason", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const drive = fakeDrive();
    drive.connect.mockRejectedValue(new Error("odd thing"));
    const { user } = await openSettings({ drive });
    const group = await target();
    await user.click(within(group).getByRole("button", { name: connectLabel }));
    expect(await within(group).findByRole("alert")).toHaveTextContent(t("security.scheduled.drive.error.other", { message: "odd thing" }));
  });
});

describe("a notice", () => {
  it("is forgotten when Settings is left, so that it is not there on the next visit", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const drive = fakeDrive();
    drive.connect.mockRejectedValueOnce(driveFailure("denied"));
    const { user } = await openSettings({ drive });
    const group = await target();
    await user.click(within(group).getByRole("button", { name: connectLabel }));
    expect(await within(group).findByRole("alert")).toHaveTextContent(t("security.scheduled.drive.error.denied"));

    await openScreen(user, "nav.home");
    await openScreen(user, "nav.settings");
    const again = await target();
    expect(within(again).queryByRole("alert")).not.toBeInTheDocument();
    expect(within(again).getByRole("button", { name: connectLabel })).toBeEnabled();
  });
});

describe("connected", () => {
  it("opens the folder in Drive through the shell's link opener", async () => {
    const opened = vi.spyOn(window, "open").mockImplementation(() => null);
    const { user } = await openSettings({ drive: fakeDrive(connectedStatus()) });
    const group = await target();
    await user.click(within(group).getByRole("button", { name: t("security.scheduled.drive.openFolder") }));
    expect(opened).toHaveBeenCalledWith(DRIVE_FOLDER_URL, "_blank", "noopener,noreferrer");
  });

  it("has no folder button without a folder address, and says 'connected' without an address", async () => {
    await openSettings({ drive: fakeDrive({ connected: true }) });
    const group = await target();
    expect(within(group).getByText(t("security.scheduled.drive.connected"))).toBeInTheDocument();
    expect(within(group).queryByRole("button", { name: t("security.scheduled.drive.openFolder") })).not.toBeInTheDocument();
  });

  it("asks before disconnecting, and says the files stay in Drive", async () => {
    const drive = fakeDrive(connectedStatus());
    const { user } = await openSettings({ drive });
    const group = await target();
    await user.click(within(group).getByRole("button", { name: t("security.scheduled.drive.disconnect") }));

    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(t("security.scheduled.drive.disconnectTitle"))).toBeInTheDocument();
    expect(within(dialog).getByText(t("security.scheduled.drive.disconnectBody"))).toBeInTheDocument();
    // Cancel keeps the connection.
    await user.click(within(dialog).getByRole("button", { name: t("dialog.cancel") }));
    expect(drive.disconnect).not.toHaveBeenCalled();
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();

    await user.click(within(group).getByRole("button", { name: t("security.scheduled.drive.disconnect") }));
    const again = await screen.findByRole("alertdialog");
    await user.click(within(again).getByRole("button", { name: t("security.scheduled.drive.disconnect") }));
    await waitFor(() => {
      expect(drive.disconnect).toHaveBeenCalledTimes(1);
    });
    expect(await within(group).findByRole("button", { name: connectLabel })).toBeEnabled();
    expect(within(group).queryByText(t("security.scheduled.drive.connectedAs", { account: DRIVE_ACCOUNT }))).not.toBeInTheDocument();
    expect(schedule().enabled).toBeDisabled();
  });

  it("shows why a disconnect failed", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const drive = fakeDrive(connectedStatus());
    drive.disconnect.mockRejectedValue(driveFailure("network"));
    const { user } = await openSettings({ drive });
    const group = await target();
    await user.click(within(group).getByRole("button", { name: t("security.scheduled.drive.disconnect") }));
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: t("security.scheduled.drive.disconnect") }));
    expect(await within(group).findByRole("alert")).toHaveTextContent(t("security.scheduled.drive.error.network"));
    // Still connected.
    expect(within(group).getByText(t("security.scheduled.drive.connectedAs", { account: DRIVE_ACCOUNT }))).toBeInTheDocument();
  });

  it("asks to reconnect when Google refused the stored sign-in, and keeps the schedule available", async () => {
    const drive = fakeDrive(connectedStatus({ needsReconnect: true }));
    const { user } = await openSettings({ drive });
    const group = await target();
    const banner = within(group).getByRole("group", { name: t("security.scheduled.drive.reconnectLabel") });
    expect(within(banner).getByText(t("security.scheduled.drive.reconnectText"))).toBeInTheDocument();
    expect(schedule().enabled).toBeEnabled();

    await user.click(within(banner).getByRole("button", { name: t("security.scheduled.drive.reconnect") }));
    expect(drive.connect).toHaveBeenCalledTimes(1);
    await waitFor(() => {
      expect(within(group).queryByRole("group", { name: t("security.scheduled.drive.reconnectLabel") })).not.toBeInTheDocument();
    });
    expect(drive.current().needsReconnect).toBe(false);
  });

  it("waits inside the banner while the browser is awaited", async () => {
    const drive = fakeDrive(connectedStatus({ needsReconnect: true }));
    drive.holdConnect();
    const { user } = await openSettings({ drive });
    const group = await target();
    await user.click(within(group).getByRole("button", { name: t("security.scheduled.drive.reconnect") }));
    const banner = within(group).getByRole("group", { name: t("security.scheduled.drive.reconnectLabel") });
    expect(await within(banner).findByRole("status")).toHaveTextContent(t("security.scheduled.drive.connecting"));
    expect(within(banner).getByRole("button", { name: t("security.scheduled.drive.cancel") })).toHaveFocus();
    expect(within(group).getByRole("button", { name: t("security.scheduled.drive.disconnect") })).toBeDisabled();
  });

  it("says nothing was uploaded yet", async () => {
    await openSettings({ drive: fakeDrive(connectedStatus()) });
    const group = await target();
    expect(within(group).getByText(t("security.scheduled.drive.last.none"))).toBeInTheDocument();
  });

  it("names the file and the time of the last upload", async () => {
    const at = Date.UTC(2026, 9, 19, 9, 30);
    await openSettings({ drive: fakeDrive(connectedStatus({ last: { at, ok: true, lastSuccessAt: at, file: FILE } })) });
    const group = await target();
    expect(
      within(group).getByText(t("security.scheduled.drive.last.ok", { date: formatTimestamp(at)?.text ?? "", file: FILE })),
    ).toBeInTheDocument();
  });

  it("says why the last upload failed, in the shell's own words", async () => {
    const at = Date.UTC(2026, 9, 19, 9, 30);
    const last = { at, ok: false, error: "Storage: Google Drive is full" };
    await openSettings({ drive: fakeDrive(connectedStatus({ last })) });
    const group = await target();
    expect(
      within(group).getByText(t("security.scheduled.drive.last.failed", { date: formatTimestamp(at)?.text ?? "", error: last.error })),
    ).toBeInTheDocument();
  });

  it("follows the shell while Settings is open: a delivery's outcome, a refused token", async () => {
    const drive = fakeDrive(connectedStatus());
    const { emit } = await openSettings({ drive });
    const group = await target();
    expect(within(group).getByText(t("security.scheduled.drive.last.none"))).toBeInTheDocument();

    const at = Date.UTC(2026, 9, 19, 9, 30);
    drive.change(connectedStatus({ last: { at, ok: true, file: FILE } }));
    act(() => {
      emit({ type: "SettingChanged", data: { key: BACKUP_DRIVE_KEY } });
    });
    expect(
      await within(group).findByText(t("security.scheduled.drive.last.ok", { date: formatTimestamp(at)?.text ?? "", file: FILE })),
    ).toBeInTheDocument();

    drive.change(connectedStatus({ needsReconnect: true }));
    act(() => {
      emit({ type: "SettingChanged", data: { key: BACKUP_DRIVE_KEY } });
    });
    expect(await within(group).findByRole("group", { name: t("security.scheduled.drive.reconnectLabel") })).toBeInTheDocument();
  });
});

describe("without a Google client", () => {
  it("says so, cannot connect, and opens Advanced", async () => {
    await openSettings({ drive: fakeDrive({ client: "none" }) });
    const group = await target();
    const text = within(group).getByText(t("security.scheduled.drive.noClient"));
    expect(text).toBeInTheDocument();
    const connect = within(group).getByRole("button", { name: connectLabel });
    expect(connect).toBeDisabled();
    expect(connect).toHaveAccessibleDescription(t("security.scheduled.drive.noClient"));
    const advanced = within(group).getByText(t("security.scheduled.drive.advanced")).closest("details");
    expect(advanced).toHaveAttribute("open");
    expect(within(group).getByLabelText(t("security.scheduled.drive.clientId"))).toBeVisible();
  });

  it("lets the person close Advanced all the same", async () => {
    const { user } = await openSettings({ drive: fakeDrive({ client: "none" }) });
    const group = await target();
    await user.click(within(group).getByText(t("security.scheduled.drive.advanced")));
    await waitFor(() => {
      expect(within(group).getByText(t("security.scheduled.drive.advanced")).closest("details")).not.toHaveAttribute("open");
    });
  });
});

describe("Advanced: one's own Google client", () => {
  async function openAdvanced(drive: FakeDrive) {
    const ctx = await openSettings({ drive });
    const group = await target();
    await ctx.user.click(within(group).getByText(t("security.scheduled.drive.advanced")));
    await waitFor(() => {
      expect(within(group).getByText(t("security.scheduled.drive.advanced")).closest("details")).toHaveAttribute("open");
    });
    return { ...ctx, group };
  }

  it("points to the operations guide by name, with no link", async () => {
    const { group } = await openAdvanced(fakeDrive());
    const help = within(group).getByText(t("security.scheduled.drive.ownHelp"));
    expect(help).toHaveTextContent("Setting up the Google client for Drive backups");
    expect(help.querySelector("a")).toBeNull();
    expect(group.querySelector("a")).toBeNull();
  });

  it("saves the id and the secret, clears the secret, and disconnects through the shell", async () => {
    const drive = fakeDrive(connectedStatus());
    const { user, group } = await openAdvanced(drive);
    const save = within(group).getByRole("button", { name: t("security.scheduled.drive.saveClient") });
    expect(save).toBeDisabled();
    expect(within(group).queryByRole("button", { name: t("security.scheduled.drive.useBuiltIn") })).not.toBeInTheDocument();

    const secret = within(group).getByLabelText(t("security.scheduled.drive.clientSecret"));
    expect(secret).toHaveAttribute("type", "password");
    await user.type(within(group).getByLabelText(t("security.scheduled.drive.clientId")), "  my-client.apps  ");
    await user.type(secret, "shh-secret");
    await user.click(save);

    expect(drive.setClient).toHaveBeenCalledWith({ clientId: "my-client.apps", clientSecret: "shh-secret" });
    expect(await within(group).findByText(t("security.scheduled.drive.ownInUse"))).toBeInTheDocument();
    expect(within(group).getByLabelText(t("security.scheduled.drive.clientId"))).toHaveValue("my-client.apps");
    expect(within(group).getByLabelText(t("security.scheduled.drive.clientSecret"))).toHaveValue("");
    // Changing the client disconnected Drive.
    expect(within(group).getByRole("button", { name: connectLabel })).toBeEnabled();
    expect(within(group).queryByText(t("security.scheduled.drive.connectedAs", { account: DRIVE_ACCOUNT }))).not.toBeInTheDocument();
  });

  it("sends no secret when none was typed", async () => {
    const drive = fakeDrive();
    const { user, group } = await openAdvanced(drive);
    await user.type(within(group).getByLabelText(t("security.scheduled.drive.clientId")), "only-an-id");
    await user.click(within(group).getByRole("button", { name: t("security.scheduled.drive.saveClient") }));
    expect(drive.setClient).toHaveBeenCalledWith({ clientId: "only-an-id" });
  });

  it("keeps what was typed when the client could not be set", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const drive = fakeDrive();
    drive.setClient.mockRejectedValue(driveFailure("server"));
    const { user, group } = await openAdvanced(drive);
    await user.type(within(group).getByLabelText(t("security.scheduled.drive.clientId")), "my-client");
    await user.type(within(group).getByLabelText(t("security.scheduled.drive.clientSecret")), "shh");
    await user.click(within(group).getByRole("button", { name: t("security.scheduled.drive.saveClient") }));
    expect(await within(group).findByRole("alert")).toHaveTextContent(t("security.scheduled.drive.error.server"));
    expect(within(group).getByLabelText(t("security.scheduled.drive.clientId"))).toHaveValue("my-client");
    expect(within(group).getByLabelText(t("security.scheduled.drive.clientSecret"))).toHaveValue("shh");
  });

  it("goes back to the built-in client", async () => {
    const drive = fakeDrive({ client: "own", ownClientId: "my-client" });
    const { user, group } = await openAdvanced(drive);
    expect(within(group).getByLabelText(t("security.scheduled.drive.clientId"))).toHaveValue("my-client");
    await user.click(within(group).getByRole("button", { name: t("security.scheduled.drive.useBuiltIn") }));
    expect(drive.setClient).toHaveBeenCalledWith(null);
    await waitFor(() => {
      expect(within(group).queryByRole("button", { name: t("security.scheduled.drive.useBuiltIn") })).not.toBeInTheDocument();
    });
    expect(within(group).getByLabelText(t("security.scheduled.drive.clientId"))).toHaveValue("");
    expect(within(group).queryByText(t("security.scheduled.drive.ownInUse"))).not.toBeInTheDocument();
  });
});

describe("the rule that turns the schedule on", () => {
  it("needs a kit first, then a folder or a connected Drive", () => {
    const all = [true, false].flatMap((hasKit) =>
      [FOLDER, null].flatMap((folder) =>
        [true, false].map((driveConnected) => ({ hasKit, folder, driveConnected, blocker: scheduleBlocker({ hasKit, folder, driveConnected }) })),
      ),
    );
    expect(all.map(({ hasKit, folder, driveConnected, blocker }) => [hasKit, folder !== null, driveConnected, blocker])).toEqual([
      [true, true, true, null],
      [true, true, false, null],
      [true, false, true, null],
      [true, false, false, "target"],
      [false, true, true, "kit"],
      [false, true, false, "kit"],
      [false, false, true, "kit"],
      [false, false, false, "kit"],
    ]);
  });

  it("is on with a folder alone, as before", async () => {
    await openSettings({ settings: { [BACKUP_FOLDER_KEY]: FOLDER } });
    await target();
    const { enabled, now } = schedule();
    expect(enabled).toBeEnabled();
    expect(now).toBeEnabled();
  });

  it("is off without a kit even when a Drive is connected, and says so", async () => {
    await openSettings({ drive: fakeDrive(connectedStatus()), kit: false });
    await target();
    expect(schedule().enabled).toBeDisabled();
    expect(schedule().now).toBeDisabled();
    expect(within(section()).getByText(t("security.backup.needsKit"))).toBeInTheDocument();
    expect(within(section()).queryByText(t("security.scheduled.drive.needsTarget"))).not.toBeInTheDocument();
  });
});

describe("Back up now", () => {
  it("says it was uploaded to Google Drive when Drive is the only target, and reads the Drive status again", async () => {
    const drive = fakeDrive(connectedStatus());
    const { user, backupRunNow } = await openSettings({ drive });
    await target();
    const before = drive.status.mock.calls.length;
    await user.click(schedule().now);
    expect(await within(section()).findByText(t("security.scheduled.drive.doneDrive", { file: FILE }))).toBeInTheDocument();
    expect(backupRunNow).toHaveBeenCalledTimes(1);
    await waitFor(() => {
      expect(drive.status.mock.calls.length).toBeGreaterThan(before);
    });
  });

  it("says it went to both with a folder and a connected Drive", async () => {
    const { user } = await openSettings({ drive: fakeDrive(connectedStatus()), settings: { [BACKUP_FOLDER_KEY]: FOLDER } });
    await target();
    await user.click(schedule().now);
    expect(await within(section()).findByText(t("security.scheduled.drive.doneBoth", { file: FILE }))).toBeInTheDocument();
  });

  it("keeps the folder-only words when Drive is not connected", async () => {
    const { user } = await openSettings({ settings: { [BACKUP_FOLDER_KEY]: FOLDER } });
    await target();
    await user.click(schedule().now);
    expect(await within(section()).findByText(t("security.scheduled.done", { file: FILE }))).toBeInTheDocument();
  });

  it("words a Drive failure by its reason", async () => {
    const run: Run = vi.fn(() => Promise.reject(driveFailure("quota")));
    const { user } = await openSettings({ drive: fakeDrive(connectedStatus()), run });
    await target();
    await user.click(schedule().now);
    expect(await within(section()).findByRole("alert")).toHaveTextContent(
      t("security.scheduled.error", { message: t("security.scheduled.drive.error.quota") }),
    );
    expect(within(section()).queryByText(t("security.backup.needsKit"))).not.toBeInTheDocument();
  });
});
