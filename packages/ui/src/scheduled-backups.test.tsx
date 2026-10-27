// Settings -> Security -> Scheduled backups (ADR-0036, section 5.4): hidden without the shell's calls, off without
// a recovery kit or a folder, the folder chosen through the shell, the schedule written to
// `app.backup.schedule` (and nothing else: the folder and the last result are the shell's), the last result
// read from `app.backup.last` and kept fresh, and "Back up now". The shell's side is the memory core's own
// (its folder dialog, its backup run and the settings it writes); a test replaces a call to make it fail.

import { CoreClient, CoreError } from "@atomik-kb/core-client";
import type { JsonValue, MemoryTransportOptions, Request, Transport } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { formatTimestamp } from "./format";
import { t } from "./i18n";
import { parseKeep } from "./screens/ScheduledBackups";
import { BACKUP_FOLDER_KEY, BACKUP_LAST_KEY, BACKUP_SCHEDULE_KEY } from "./store-interop";
import { backupResult, renderInterop } from "./test/interop";
import type { InteropMethods } from "./test/interop";
import { NOW } from "./test/render";

afterEach(() => {
  vi.restoreAllMocks();
});

const KIT: MemoryTransportOptions["security"] = { kitCreatedAt: NOW, kitKey: new Uint8Array(32).fill(3) };
const FOLDER = "C:\\Users\\Ada\\Backups";
/** The folder the memory core's folder dialog picks. */
const MEMORY_FOLDER = "/backups/scheduled";
/** The name of a scheduled backup (local time, so only its form is known). */
const AUTO_FILE = "atomik-kb-auto-\\d{4}-\\d{2}-\\d{2}-\\d{4}\\.atomikkb";

type FolderChoose = NonNullable<Transport["backupFolderChoose"]>;
type RunNow = NonNullable<Transport["backupRunNow"]>;

interface Setup {
  settings?: Record<string, JsonValue>;
  kit?: boolean;
  /** Replaces the shell's scheduled backup calls (`null`: a shell without them). */
  methods?: InteropMethods | null;
}

/**
 * Settings over a memory core whose folder dialog and "Back up now" are the memory core's own (with spies
 * around them), or the calls a test gives, or none.
 */
async function openSettings({ settings = {}, kit = true, methods }: Setup = {}) {
  const backupFolderChoose = vi.fn<FolderChoose>();
  const backupRunNow = vi.fn<RunNow>();
  const ctx = await renderInterop(
    { workspaceName: "Research", settings, security: kit ? KIT : undefined, initialRoute: "settings" },
    {
      methods:
        methods === null
          ? {}
          : (methods ??
            ((memory) => {
              backupFolderChoose.mockImplementation(() => memory.backupFolderChoose());
              backupRunNow.mockImplementation(() => memory.backupRunNow());
              return { backupFolderChoose, backupRunNow };
            })),
    },
  );
  const client = new CoreClient(ctx.memory);
  await screen.findByRole("heading", { name: t("settings.title") });
  return { ...ctx, client, backupFolderChoose, backupRunNow };
}

function section(): HTMLElement {
  return screen.getByRole("region", { name: t("security.scheduled.title") });
}

function controls() {
  const part = within(section());
  return {
    enabled: part.getByRole("checkbox", { name: t("security.scheduled.enabled") }),
    interval: part.getByRole("combobox", { name: t("security.scheduled.interval") }),
    keep: part.getByRole("spinbutton", { name: t("security.scheduled.keep") }),
    choose: part.getByRole("button", { name: t("security.scheduled.choose") }),
    now: part.getByRole("button", { name: t("security.scheduled.now") }),
  };
}

/** The schedule stored in the core. */
async function stored(client: CoreClient): Promise<JsonValue | null> {
  return client.settingsGet(BACKUP_SCHEDULE_KEY);
}

function settingKeysWritten(requests: readonly Request[]): string[] {
  return requests.flatMap((request) => (request.type === "SettingsSet" ? [request.data.key] : []));
}

describe("the section", () => {
  it("is not shown where the shell cannot run scheduled backups", async () => {
    await openSettings({ methods: null });
    expect(screen.getByRole("region", { name: t("security.backup.title") })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: t("security.scheduled.title") })).not.toBeInTheDocument();
  });

  it("is off until a recovery kit exists, as the manual backup is", async () => {
    await openSettings({ kit: false, settings: { [BACKUP_FOLDER_KEY]: FOLDER } });
    const { enabled, interval, keep, now, choose } = controls();
    const note = within(section()).getByText(t("security.backup.needsKit"));
    for (const control of [enabled, interval, keep, now]) {
      expect(control).toBeDisabled();
    }
    expect(enabled).toHaveAccessibleDescription(t("security.backup.needsKit"));
    expect(now).toHaveAccessibleDescription(t("security.backup.needsKit"));
    expect(note).toBeInTheDocument();
    // The folder can be chosen before the kit exists.
    expect(choose).toBeEnabled();
  });

  it("is off without a folder, until one is chosen through the shell", async () => {
    const { user, backupFolderChoose } = await openSettings();
    const before = controls();
    expect(within(section()).getByText(t("security.scheduled.noFolder"))).toBeInTheDocument();
    expect(within(section()).getByText(t("security.scheduled.needsFolder"))).toBeInTheDocument();
    for (const control of [before.enabled, before.interval, before.keep, before.now]) {
      expect(control).toBeDisabled();
    }

    await user.click(before.choose);
    await waitFor(() => {
      expect(within(section()).getByText(MEMORY_FOLDER)).toBeInTheDocument();
    });
    expect(backupFolderChoose).toHaveBeenCalledTimes(1);
    const after = controls();
    for (const control of [after.enabled, after.interval, after.keep, after.now]) {
      expect(control).toBeEnabled();
    }
    expect(within(section()).queryByText(t("security.scheduled.needsFolder"))).not.toBeInTheDocument();
  });

  it("says so, and changes nothing, when the folder dialog is cancelled", async () => {
    const backupFolderChoose = vi.fn<FolderChoose>(() =>
      Promise.reject(new CoreError({ code: "InvalidInput", message: "no folder was chosen", details: { field: "path" } })),
    );
    const { user } = await openSettings({ methods: { backupFolderChoose, backupRunNow: () => Promise.resolve(backupResult()) } });
    await user.click(controls().choose);
    expect(await within(section()).findByRole("status")).toHaveTextContent(t("security.scheduled.cancelled"));
    expect(within(section()).getByText(t("security.scheduled.noFolder"))).toBeInTheDocument();
    expect(within(section()).queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows why the folder could not be set", async () => {
    const backupFolderChoose = vi.fn<FolderChoose>(() => Promise.reject(new CoreError({ code: "Storage", message: "no access" })));
    const { user } = await openSettings({ methods: { backupFolderChoose, backupRunNow: () => Promise.resolve(backupResult()) } });
    await user.click(controls().choose);
    expect(await within(section()).findByRole("alert")).toHaveTextContent(t("security.scheduled.folderError", { message: "no access" }));
  });
});

describe("the schedule", () => {
  it("starts off, every day, keeping seven", async () => {
    await openSettings({ settings: { [BACKUP_FOLDER_KEY]: FOLDER } });
    const { enabled, interval, keep } = controls();
    expect(enabled).not.toBeChecked();
    expect(interval).toHaveValue("daily");
    expect(keep).toHaveValue(7);
    expect(within(section()).getByText(FOLDER)).toBeInTheDocument();
  });

  it("shows what is stored", async () => {
    await openSettings({
      settings: { [BACKUP_FOLDER_KEY]: FOLDER, [BACKUP_SCHEDULE_KEY]: { enabled: true, interval: "weekly", keep: 30 } },
    });
    const { enabled, interval, keep } = controls();
    expect(enabled).toBeChecked();
    expect(interval).toHaveValue("weekly");
    expect(keep).toHaveValue(30);
  });

  it("is written to app.backup.schedule when it is turned on, and its interval and count change", async () => {
    const { user, client, requests } = await openSettings({ settings: { [BACKUP_FOLDER_KEY]: FOLDER } });
    await user.click(controls().enabled);
    await waitFor(async () => {
      expect(await stored(client)).toEqual({ enabled: true, interval: "daily", keep: 7 });
    });
    expect(controls().enabled).toBeChecked();

    await user.selectOptions(controls().interval, "weekly");
    await waitFor(async () => {
      expect(await stored(client)).toEqual({ enabled: true, interval: "weekly", keep: 7 });
    });

    const { keep } = controls();
    await user.clear(keep);
    await user.type(keep, "14");
    await user.tab();
    await waitFor(async () => {
      expect(await stored(client)).toEqual({ enabled: true, interval: "weekly", keep: 14 });
    });
    expect(controls().keep).toHaveValue(14);

    await user.click(controls().enabled);
    await waitFor(async () => {
      expect(await stored(client)).toEqual({ enabled: false, interval: "weekly", keep: 14 });
    });
    // Only the schedule is the UI's to write: the folder and the last result belong to the shell.
    expect(new Set(settingKeysWritten(requests))).toEqual(new Set([BACKUP_SCHEDULE_KEY]));
  });

  it("takes the number to keep with Enter, and refuses anything outside 1 to 100", async () => {
    const { user, client } = await openSettings({ settings: { [BACKUP_FOLDER_KEY]: FOLDER } });
    const { keep } = controls();
    await user.clear(keep);
    await user.type(keep, "30{Enter}");
    await waitFor(async () => {
      expect(await stored(client)).toMatchObject({ keep: 30 });
    });

    for (const bad of ["0", "101", "250"]) {
      await user.clear(controls().keep);
      await user.type(controls().keep, bad);
      await user.tab();
      expect(await within(section()).findByRole("alert")).toHaveTextContent(t("security.scheduled.keepError"));
      // The stored number is back in the field, and nothing was written.
      expect(controls().keep).toHaveValue(30);
      expect(await stored(client)).toMatchObject({ keep: 30 });
    }
  });

  it("puts the stored number back with Escape", async () => {
    const { user, client } = await openSettings({ settings: { [BACKUP_FOLDER_KEY]: FOLDER } });
    await user.clear(controls().keep);
    await user.type(controls().keep, "50{Escape}");
    expect(controls().keep).toHaveValue(7);
    await user.tab();
    expect(await stored(client)).toBeNull();
  });

  it("parses what was typed", () => {
    expect([" 7 ", "1", "100", "0", "101", "", "7.5", "-3", "abc", "1e2"].map(parseKeep)).toEqual([
      7,
      1,
      100,
      null,
      null,
      null,
      null,
      null,
      null,
      null,
    ]);
  });
});

describe("the last result", () => {
  const AT = Date.UTC(2026, 9, 12, 9, 30);
  const when = formatTimestamp(AT)?.text ?? "";

  it("says nothing has run yet", async () => {
    await openSettings({ settings: { [BACKUP_FOLDER_KEY]: FOLDER } });
    expect(within(section()).getByText(t("security.scheduled.last.none"))).toBeInTheDocument();
  });

  it("names the file of the last good backup and when", async () => {
    await openSettings({
      settings: {
        [BACKUP_FOLDER_KEY]: FOLDER,
        [BACKUP_LAST_KEY]: { at: AT, ok: true, lastSuccessAt: AT, file: "atomik-kb-auto-2026-10-12-0930.atomikkb" },
      },
    });
    expect(
      within(section()).getByText(t("security.scheduled.last.ok", { date: when, file: "atomik-kb-auto-2026-10-12-0930.atomikkb" })),
    ).toBeInTheDocument();
  });

  it("says why the last attempt failed, and when the last good one was", async () => {
    await openSettings({
      settings: {
        [BACKUP_FOLDER_KEY]: FOLDER,
        [BACKUP_LAST_KEY]: { at: AT, ok: false, lastSuccessAt: AT - 86_400_000, error: "Storage: the folder is gone" },
      },
    });
    expect(
      within(section()).getByText(t("security.scheduled.last.failed", { date: when, error: "Storage: the folder is gone" })),
    ).toBeInTheDocument();
    expect(
      within(section()).getByText(t("security.scheduled.last.lastGood", { date: formatTimestamp(AT - 86_400_000)?.text ?? "" })),
    ).toBeInTheDocument();
  });

  it("follows what the shell writes while Settings is open", async () => {
    const { memory } = await openSettings();
    expect(within(section()).getByText(t("security.scheduled.noFolder"))).toBeInTheDocument();
    // The shell's own side: its folder dialog stores the folder, its timer runs the backup and records it.
    await act(async () => {
      await memory.backupFolderChoose();
    });
    await waitFor(() => {
      expect(within(section()).getByText(MEMORY_FOLDER)).toBeInTheDocument();
    });
    expect(controls().now).toBeEnabled();
    await act(async () => {
      await memory.backupRunNow();
    });
    await waitFor(() => {
      expect(within(section()).getByText(new RegExp(`^Backed up .*: ${AUTO_FILE}$`))).toBeInTheDocument();
    });
  });
});

describe("Back up now", () => {
  it("asks the shell, says which file it wrote and reads the last result again", async () => {
    const { user, backupRunNow } = await openSettings({ settings: { [BACKUP_FOLDER_KEY]: MEMORY_FOLDER } });
    await user.click(controls().now);
    expect(await within(section()).findByRole("status")).toHaveTextContent(new RegExp(`^Backup written: ${AUTO_FILE}$`));
    expect(backupRunNow).toHaveBeenCalledTimes(1);
    // The shell recorded the outcome in app.backup.last, and the section shows it.
    await waitFor(() => {
      expect(within(section()).getByText(new RegExp(`^Backed up .*: ${AUTO_FILE}$`))).toBeInTheDocument();
    });
    expect(controls().now).toBeEnabled();
  });

  it("shows a busy button while the shell writes the file", async () => {
    let finish: () => void = () => undefined;
    const { user, backupRunNow } = await openSettings({ settings: { [BACKUP_FOLDER_KEY]: FOLDER } });
    backupRunNow.mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = () => {
            resolve(backupResult());
          };
        }),
    );
    await user.click(controls().now);
    const busy = within(section()).getByRole("button", { name: t("security.scheduled.running") });
    expect(busy).toBeDisabled();
    expect(within(section()).getByRole("button", { name: t("security.scheduled.choose") })).toBeDisabled();
    await act(async () => {
      finish();
      await Promise.resolve();
    });
    expect(await within(section()).findByRole("button", { name: t("security.scheduled.now") })).toBeEnabled();
  });

  it("shows why it failed", async () => {
    const { user, backupRunNow } = await openSettings({ settings: { [BACKUP_FOLDER_KEY]: FOLDER } });
    backupRunNow.mockRejectedValue(new CoreError({ code: "Storage", message: "disk full" }));
    await user.click(controls().now);
    expect(await within(section()).findByRole("alert")).toHaveTextContent(t("security.scheduled.error", { message: "disk full" }));
  });

  it("explains the recovery kit when the core refuses to back up without one", async () => {
    const { user, backupRunNow } = await openSettings({ settings: { [BACKUP_FOLDER_KEY]: FOLDER } });
    backupRunNow.mockRejectedValue(new CoreError({ code: "InvalidInput", message: "no kit", details: { field: "kit" } }));
    await user.click(controls().now);
    expect(await within(section()).findByRole("alert")).toHaveTextContent(t("security.backup.needsKit"));
  });
});
