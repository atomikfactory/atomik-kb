// Settings -> Data, attachments: the files of the current space, the download policy of
// this device (`app.attachments.download`, Milestone 8), the retention of unused files
// (`app.attachments.retention_days`) and "Clean up now".

import { CoreError, DOWNLOAD_POLICY_SETTING, MemoryTransport, RETENTION_SETTING } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { formatBytes } from "./format";
import { t, tp } from "./i18n";
import { describeCollection, parseRetentionDays } from "./screens/AttachmentSettings";
import { pngFile, textFile, upload } from "./test/attachments";
import { idsOf } from "./test/page";
import { NOW, findMainNavigation, openScreen, recordingTransport, renderApp, renderWith } from "./test/render";

const DAY_MS = 24 * 60 * 60 * 1000;

async function openSettings(options: Parameters<typeof renderApp>[0] = {}) {
  const app = renderApp({
    workspaceName: "Research",
    spaces: [{ name: "Personal", pages: [{ title: "Notes" }] }, { name: "Work" }],
    ...options,
  });
  await openScreen(app.user, "nav.settings");
  await screen.findByRole("heading", { name: t("settings.title") });
  return app;
}

const panel = (): HTMLElement => screen.getByRole("region", { name: t("settings.attachments.title") });
const retention = (): HTMLSelectElement => within(panel()).getByRole("combobox", { name: t("settings.attachments.retention") });
const downloads = (): HTMLSelectElement => within(panel()).getByRole("combobox", { name: t("settings.attachments.download") });

async function storedPolicy(memory: MemoryTransport): Promise<unknown> {
  const response = await memory.dispatch({ type: "SettingsGet", data: { key: DOWNLOAD_POLICY_SETTING } });
  return response.type === "Setting" ? response.data : response;
}

async function uploadTwo(memory: MemoryTransport): Promise<number> {
  const { space } = await idsOf(memory);
  const files = [pngFile("cat.png"), textFile("notes.txt", "some notes")];
  await act(async () => {
    for (const file of files) {
      await upload(memory, space("Personal"), file);
    }
  });
  return files.reduce((sum, file) => sum + file.size, 0);
}

describe("settings: attachments", () => {
  it("counts the files of the current space and follows uploads", async () => {
    const { memory } = await openSettings();
    expect(await within(panel()).findByText(t("settings.attachments.none"))).toBeInTheDocument();
    expect(within(panel()).getByText(t("settings.attachments.inSpace", { space: "Personal" }))).toBeInTheDocument();

    const total = await uploadTwo(memory);
    expect(
      await within(panel()).findByText(tp("settings.attachments.usage", 2, { size: formatBytes(total) })),
    ).toBeInTheDocument();
  });

  it("keeps unused files for 30 days unless another retention is chosen, and saves the choice", async () => {
    const { user, memory } = await openSettings();
    await waitFor(() => {
      expect(retention()).toBeEnabled();
    });
    expect(retention()).toHaveValue("30");
    expect(Array.from(retention().options).map((option) => option.textContent)).toEqual([
      "7 days",
      "30 days",
      "90 days",
      t("settings.attachments.retention.never"),
    ]);

    await user.selectOptions(retention(), t("settings.attachments.retention.never"));
    await waitFor(async () => {
      expect(await memory.dispatch({ type: "SettingsGet", data: { key: RETENTION_SETTING } })).toEqual({
        type: "Setting",
        data: 0,
      });
    });
    expect(retention()).toHaveValue("0");
  });

  it("shows a stored retention that is not one of the choices", async () => {
    await openSettings({ settings: { [RETENTION_SETTING]: 14 } });
    await waitFor(() => {
      expect(retention()).toHaveValue("14");
    });
    expect(within(retention()).getByRole("option", { name: "14 days" })).toBeInTheDocument();
  });

  it("cleans up files unused past the retention now, and says what it did", async () => {
    let clock = NOW;
    const { user, memory } = await openSettings({ now: () => clock });
    await uploadTwo(memory);
    expect(await within(panel()).findByText(/^2 files/)).toBeInTheDocument();
    const clean = within(panel()).getByRole("button", { name: t("settings.attachments.clean") });

    // Within the retention: nothing goes.
    await user.click(clean);
    expect(await within(panel()).findByText(t("settings.attachments.clean.nothing"))).toBeInTheDocument();

    clock = NOW + 31 * DAY_MS;
    await user.click(clean);
    expect(await within(panel()).findByText(/^Cleaned up: 2 unused files deleted, .+ freed\.$/)).toBeInTheDocument();
    expect(await within(panel()).findByText(t("settings.attachments.none"))).toBeInTheDocument();
  });

  it("never collects with the retention set to forever", async () => {
    let clock = NOW;
    const { user, memory } = await openSettings({ now: () => clock, settings: { [RETENTION_SETTING]: 0 } });
    await uploadTwo(memory);
    clock = NOW + 3650 * DAY_MS;
    await user.click(within(panel()).getByRole("button", { name: t("settings.attachments.clean") }));
    expect(await within(panel()).findByText(t("settings.attachments.clean.nothing"))).toBeInTheDocument();
    expect(within(panel()).getByText(/^2 files/)).toBeInTheDocument();
  });

  it("reads the stored retention and words the clean-up summary", () => {
    expect([parseRetentionDays(undefined), parseRetentionDays(null), parseRetentionDays(7), parseRetentionDays(0)]).toEqual([
      30, 30, 7, 0,
    ]);
    expect([parseRetentionDays(-1), parseRetentionDays(2.5), parseRetentionDays("7")]).toEqual([30, 30, 30]);
    expect(describeCollection({ purged: 0, chunksDeleted: 0, bytesFreed: 0 })).toBe(t("settings.attachments.clean.nothing"));
    expect(describeCollection({ purged: 1, chunksDeleted: 3, bytesFreed: 2048 })).toBe(
      t("settings.attachments.clean.done", { count: 1, size: formatBytes(2048) }),
    );
  });
});

describe("settings: attachments, download policy", () => {
  it("downloads automatically unless set to when opened, and saves the choice", async () => {
    const { user, memory } = await openSettings();
    await waitFor(() => {
      expect(downloads()).toBeEnabled();
    });
    expect(downloads()).toHaveValue("eager");
    expect(Array.from(downloads().options).map((option) => option.textContent)).toEqual([
      t("settings.attachments.download.eager"),
      t("settings.attachments.download.onDemand"),
    ]);
    expect(downloads()).toHaveAccessibleDescription(t("settings.attachments.download.hint"));
    expect(await storedPolicy(memory)).toBeNull();

    await user.selectOptions(downloads(), t("settings.attachments.download.onDemand"));
    await waitFor(async () => {
      expect(await storedPolicy(memory)).toBe("on_demand");
    });
    expect(downloads()).toHaveValue("on_demand");

    await user.selectOptions(downloads(), t("settings.attachments.download.eager"));
    await waitFor(async () => {
      expect(await storedPolicy(memory)).toBe("eager");
    });
  });

  it("shows the stored policy, and follows a change made elsewhere", async () => {
    const { memory } = await openSettings({ settings: { [DOWNLOAD_POLICY_SETTING]: "on_demand" } });
    await waitFor(() => {
      expect(downloads()).toHaveValue("on_demand");
    });
    await act(async () => {
      await memory.dispatch({ type: "SettingsSet", data: { key: DOWNLOAD_POLICY_SETTING, value: "eager" } });
    });
    await waitFor(() => {
      expect(downloads()).toHaveValue("eager");
    });
  });

  it("says why the choice could not be saved", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", spaces: [{ name: "Personal" }] });
    const recording = recordingTransport(memory);
    const dispatch = recording.transport.dispatch.bind(recording.transport);
    recording.transport.dispatch = (request) =>
      request.type === "SettingsSet" && request.data.key === DOWNLOAD_POLICY_SETTING
        ? Promise.reject(new CoreError({ code: "Storage", message: "disk I/O error" }))
        : dispatch(request);
    const { user } = renderWith(recording.transport);
    await findMainNavigation();
    await openScreen(user, "nav.settings");
    await waitFor(() => {
      expect(downloads()).toBeEnabled();
    });
    await user.selectOptions(downloads(), t("settings.attachments.download.onDemand"));
    expect(await within(panel()).findByRole("alert")).toHaveTextContent(
      t("settings.attachments.download.error", { message: "disk I/O error" }),
    );
    expect(downloads()).toHaveValue("eager");
  });

  it("shows a stored value it does not know as the default", async () => {
    await openSettings({ settings: { [DOWNLOAD_POLICY_SETTING]: "lazy" } });
    await waitFor(() => {
      expect(downloads()).toBeEnabled();
    });
    expect(downloads()).toHaveValue("eager");
  });
});
