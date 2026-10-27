import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { MemoryTransportOptions, Transport } from "@atomik-kb/core-client";
import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import { NOW, findMainNavigation, openScreen, recordingTransport, renderWith } from "./test/render";
import type { User } from "./test/render";
import { PASSPHRASE } from "./test/security";

const SERVER = "https://sync.example.com";
const KIT = `${"abandon ".repeat(23)}art`;
/** What the mirror's imaginary devices show (see `devices-mirror.ts` in core-client). */
const CODE_GROUPS = ["ABCD", "EFGH", "JKMN"];
const DIGITS = ["428", "516"];
const JOINER = "Pixel 9";
const EXPIRED_CODE = "BADB ADBA DBAD";

const ACCOUNT: MemoryTransportOptions = {
  workspaceName: "Research",
  security: { kitCreatedAt: NOW },
  sync: { state: "idle", serverUrl: SERVER },
};

afterEach(() => {
  vi.restoreAllMocks();
});

/** The app over the memory core (pairing steps of `stepMs`), recording requests. */
function renderDevices(options: MemoryTransportOptions = {}, wrap: (transport: Transport) => Transport = (x) => x) {
  const memory = new MemoryTransport({ now: () => NOW, pairingStepMs: 60, syncStepMs: 5, ...options });
  const recording = recordingTransport(memory);
  return { memory, requests: recording.requests, ...renderWith(wrap(recording.transport)) };
}

async function onboardingCard(): Promise<HTMLElement> {
  return screen.findByRole("region", { name: t("onboarding.title") });
}

/** The groups shown by a `role="group"` display (the code, the digits). */
function groupsOf(label: string): string[] {
  return spansOf(screen.getByRole("group", { name: label }));
}

/** The groups of a display found earlier (it may already have been replaced by the next step). */
function spansOf(group: HTMLElement): string[] {
  return Array.from(group.querySelectorAll("span")).map((span) => span.textContent ?? "");
}

async function openDevices(user: User): Promise<HTMLElement> {
  await openScreen(user, "nav.settings");
  await screen.findByRole("heading", { name: t("settings.title") });
  return screen.getByRole("region", { name: t("security.devices.title") });
}

describe("onboarding: add this device to my account (joiner)", () => {
  it("offers the two other ways in next to creating and restoring", async () => {
    renderDevices();
    const card = await onboardingCard();
    expect(card).toHaveTextContent(t("onboarding.other.intro"));
    expect(within(card).getByRole("button", { name: t("onboarding.join.button") })).toBeEnabled();
    expect(within(card).getByRole("button", { name: t("onboarding.recover.button") })).toBeEnabled();
  });

  it("walks the address, the code, the digits and lands in the workspace", async () => {
    // Slower pairing steps than the other tests: under the load of the whole suite, 60 ms could pass
    // between the code appearing and this test reading it.
    const { user, requests } = renderDevices({ pairingStepMs: 400 });
    await user.click(within(await onboardingCard()).getByRole("button", { name: t("onboarding.join.button") }));
    const flow = screen.getByRole("region", { name: t("join.title") });
    expect(flow).toHaveTextContent(t("join.intro"));

    await user.click(within(flow).getByRole("button", { name: t("join.connect") }));
    expect(within(flow).getByRole("alert")).toHaveTextContent(t("join.error.emptyUrl"));
    await user.type(within(flow).getByRole("textbox", { name: t("join.urlLabel") }), "ftp://nope");
    await user.click(within(flow).getByRole("button", { name: t("join.connect") }));
    expect(await within(flow).findByRole("alert")).toHaveTextContent(/^This address cannot be used: .+\.$/);

    const field = within(flow).getByRole("textbox", { name: t("join.urlLabel") });
    await user.clear(field);
    await user.type(field, SERVER);
    await user.click(within(flow).getByRole("button", { name: t("join.connect") }));

    const code = await screen.findByRole("group", { name: t("join.codeLabel") });
    expect(spansOf(code)).toEqual(CODE_GROUPS);
    expect(flow).toHaveTextContent(t("join.codeIntro"));
    expect(flow).toHaveTextContent(t("join.waiting"));
    expect(flow).toHaveTextContent(/The code works until .+\./);

    const digits = await screen.findByRole("group", { name: t("pair.digitsLabel") }, { timeout: 3000 });
    expect(spansOf(digits)).toEqual(DIGITS);
    expect(screen.getByRole("group", { name: t("pair.digitsLabel") })).toHaveAccessibleDescription(t("pair.warning"));
    expect(flow).toHaveTextContent(
      "Only confirm if the other device shows the same digits. Nobody from atomik-kb will ever ask you for them.",
    );
    await user.click(within(flow).getByRole("button", { name: t("pair.confirm") }));

    await screen.findByRole("navigation", { name: t("nav.label") }, { timeout: 4000 });
    expect(requests.map((request) => request.type)).toEqual(expect.arrayContaining(["PairingStart", "PairingConfirm"]));
    expect(requests).toContainEqual({ type: "PairingStart", data: { role: "Joiner", serverUrl: SERVER } });
  });

  it("cancels while waiting, and Back returns to creating a workspace", async () => {
    const { user } = renderDevices();
    await user.click(within(await onboardingCard()).getByRole("button", { name: t("onboarding.join.button") }));
    const flow = screen.getByRole("region", { name: t("join.title") });
    await user.type(within(flow).getByRole("textbox", { name: t("join.urlLabel") }), SERVER);
    await user.click(within(flow).getByRole("button", { name: t("join.connect") }));
    await screen.findByRole("group", { name: t("join.codeLabel") });
    await user.click(within(flow).getByRole("button", { name: t("pair.cancel") }));

    expect(await within(flow).findByRole("status")).toHaveTextContent(t("join.cancelled"));
    expect(within(flow).getByRole("textbox", { name: t("join.urlLabel") })).toHaveValue(SERVER);
    await user.click(within(flow).getByRole("button", { name: t("onboarding.back") }));
    expect(await onboardingCard()).toBeInTheDocument();
  });

  it("picks a session under way up again when the app starts", async () => {
    const memory = new MemoryTransport({ now: () => NOW, pairingStepMs: 60 });
    await memory.dispatch({ type: "PairingStart", data: { role: "Joiner", serverUrl: SERVER } });
    renderWith(memory);
    const flow = await screen.findByRole("region", { name: t("join.title") });
    expect(await within(flow).findByRole("group", { name: t("join.codeLabel") })).toBeInTheDocument();
  });
});

describe("onboarding: recover with my kit", () => {
  it("recovers with the server and the kit, then asks to re-issue the kit", async () => {
    const { user, requests } = renderDevices();
    await user.click(within(await onboardingCard()).getByRole("button", { name: t("onboarding.recover.button") }));
    const flow = screen.getByRole("region", { name: t("recover.title") });
    expect(flow).toHaveTextContent(t("recover.intro"));

    await user.type(within(flow).getByRole("textbox", { name: t("recover.urlLabel") }), SERVER);
    await user.click(within(flow).getByRole("button", { name: t("recover.submit") }));
    expect(within(flow).getByRole("alert")).toHaveTextContent(t("recover.error.emptyKit"));

    await user.type(within(flow).getByRole("textbox", { name: t("recover.kitLabel") }), KIT);
    expect(flow).toHaveTextContent(t("restoreKit.hint.count", { count: 24 }));
    await user.click(within(flow).getByRole("button", { name: t("recover.submit") }));
    expect(await within(flow).findByText(t("recover.fetching"))).toBeInTheDocument();

    await findMainNavigation();
    expect(requests).toContainEqual({ type: "RecoveryStart", data: { serverUrl: SERVER, kit: KIT } });
    const banner = await screen.findByRole("complementary", { name: t("staleKitBanner.label") });
    expect(banner).toHaveTextContent(t("staleKitBanner.body"));

    await user.click(within(banner).getByRole("button", { name: t("staleKitBanner.action") }));
    const dialog = await screen.findByRole("dialog", { name: t("security.reissue.title") });
    await user.click(within(dialog).getByRole("button", { name: t("security.reissue.confirm") }));
    await screen.findByRole("dialog", { name: t("security.reissue.newTitle") });
    await waitFor(() => {
      expect(screen.queryByRole("complementary", { name: t("staleKitBanner.label") })).not.toBeInTheDocument();
    });
  });

  it("lets the user dismiss the re-issue reminder on this device without re-issuing", async () => {
    const { user, requests } = renderDevices();
    await user.click(within(await onboardingCard()).getByRole("button", { name: t("onboarding.recover.button") }));
    const flow = screen.getByRole("region", { name: t("recover.title") });
    await user.type(within(flow).getByRole("textbox", { name: t("recover.urlLabel") }), SERVER);
    await user.type(within(flow).getByRole("textbox", { name: t("recover.kitLabel") }), KIT);
    await user.click(within(flow).getByRole("button", { name: t("recover.submit") }));

    await findMainNavigation();
    const banner = await screen.findByRole("complementary", { name: t("staleKitBanner.label") });
    await user.click(within(banner).getByRole("button", { name: t("staleKitBanner.dismiss") }));
    await waitFor(() => {
      expect(screen.queryByRole("complementary", { name: t("staleKitBanner.label") })).not.toBeInTheDocument();
    });
    expect(requests.some((request) => request.type === "RecoveryKitReissue")).toBe(false);
  });

  it("says why when the server does not know the kit, and when the kit cannot be read", async () => {
    const { user } = renderDevices();
    await user.click(within(await onboardingCard()).getByRole("button", { name: t("onboarding.recover.button") }));
    const flow = screen.getByRole("region", { name: t("recover.title") });
    await user.type(within(flow).getByRole("textbox", { name: t("recover.urlLabel") }), SERVER);
    await user.type(within(flow).getByRole("textbox", { name: t("recover.kitLabel") }), `wrong ${KIT}`);
    await user.click(within(flow).getByRole("button", { name: t("recover.submit") }));
    expect(await within(flow).findByRole("alert")).toHaveTextContent(/^Recovery stopped: .+/);
    // The form is back, without the kit.
    expect(within(flow).getByRole("textbox", { name: t("recover.urlLabel") })).toHaveValue(SERVER);
    expect(within(flow).getByRole("textbox", { name: t("recover.kitLabel") })).toHaveValue("");

    await user.type(within(flow).getByRole("textbox", { name: t("recover.kitLabel") }), "abandon abandon zebra");
    await user.click(within(flow).getByRole("button", { name: t("recover.submit") }));
    expect(await within(flow).findByText(t("recover.error.kit"))).toBeInTheDocument();
  });
});

/**
 * Reports `RecoveryKitInfo.stale` as the core does (the mirror leaves it out): `true` until a
 * re-issue went through when `initially` is `true`, `false` throughout otherwise.
 */
function reportingStale(initially: boolean) {
  return (transport: Transport): Transport => {
    let stale = initially;
    return {
      ...transport,
      dispatch: async (request) => {
        if (request.type === "RecoveryKitReissue") {
          // Before the answer: the core's SecurityChanged (and the read it causes) comes first.
          stale = false;
        }
        const response = await transport.dispatch(request);
        if (response.type === "Security" && response.data.recoveryKit !== undefined) {
          return { type: "Security", data: { ...response.data, recoveryKit: { ...response.data.recoveryKit, stale } } };
        }
        return response;
      },
    };
  };
}

describe("re-issue banner: the core's stale flag", () => {
  it("shows while the core says the kit is stale, on any device, and goes after a re-issue", async () => {
    const { user } = renderDevices({ ...ACCOUNT, devices: { devices: 2 } }, reportingStale(true));
    const banner = await screen.findByRole("complementary", { name: t("staleKitBanner.label") });
    await user.click(within(banner).getByRole("button", { name: t("staleKitBanner.action") }));
    const dialog = await screen.findByRole("dialog", { name: t("security.reissue.title") });
    await user.click(within(dialog).getByRole("button", { name: t("security.reissue.confirm") }));
    await screen.findByRole("dialog", { name: t("security.reissue.newTitle") });
    await waitFor(() => {
      expect(screen.queryByRole("complementary", { name: t("staleKitBanner.label") })).not.toBeInTheDocument();
    });
  });

  it("wins over how the device joined: not stale after a recovery means no banner", async () => {
    const memory = new MemoryTransport({ now: () => NOW, pairingStepMs: 1, syncStepMs: 5 });
    const client = new CoreClient(memory);
    await client.recoveryStart(SERVER, KIT);
    await vi.waitFor(async () => {
      expect((await client.recoveryStatus()).state).toBe("Done");
    });
    expect((await client.deviceList()).find((device) => device.isThisDevice)?.addedViaKit).toBe(true);
    renderWith(reportingStale(false)(recordingTransport(memory).transport));
    await findMainNavigation();
    // Without the flag the fallback would ask (see the recovery test above); the core said no.
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(screen.queryByRole("complementary", { name: t("staleKitBanner.label") })).not.toBeInTheDocument();
  });
});

describe("settings: add a device (sponsor)", () => {
  it("types the code in any form, compares the digits, sees the name, confirms, and the device is listed", async () => {
    const { user, requests } = renderDevices({ ...ACCOUNT, devices: { devices: 2 } });
    const devices = await openDevices(user);
    expect(devices).toHaveTextContent(t("security.devices.addNote"));
    await user.click(within(devices).getByRole("button", { name: t("security.devices.add") }));
    const dialog = await screen.findByRole("dialog", { name: t("addDevice.title") });
    const field = within(dialog).getByRole("textbox", { name: t("addDevice.codeLabel") });
    expect(field).toHaveFocus();

    await user.type(field, "abcd-efgh");
    await user.click(within(dialog).getByRole("button", { name: t("addDevice.submit") }));
    expect(within(dialog).getByRole("alert")).toHaveTextContent(t("addDevice.error.length", { count: 8 }));
    await user.type(field, " jkmn");
    await user.click(within(dialog).getByRole("button", { name: t("addDevice.submit") }));

    await within(dialog).findByRole("group", { name: t("pair.digitsLabel") });
    expect(groupsOf(t("pair.digitsLabel"))).toEqual(DIGITS);
    expect(within(dialog).queryByRole("button", { name: t("pair.confirm") })).not.toBeInTheDocument();
    expect(dialog).toHaveTextContent(t("addDevice.waitingPeer"));

    expect(await within(dialog).findByText(t("addDevice.adding", { name: JOINER }))).toBeInTheDocument();
    expect(dialog).toHaveTextContent(t("pair.warning"));
    await user.click(within(dialog).getByRole("button", { name: t("pair.confirm") }));
    expect(await within(dialog).findByText(t("addDevice.done", { name: JOINER }))).toHaveAttribute("role", "status");
    expect(requests).toContainEqual({ type: "PairingStart", data: { role: "Sponsor", code: "ABCDEFGHJKMN" } });
    await user.click(within(dialog).getByRole("button", { name: t("kit.done") }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    await waitFor(() => {
      expect(devices).toHaveTextContent(JOINER);
    });
    const thisName = within(devices)
      .getAllByRole("listitem")
      .find((item) => within(item).queryByText(t("security.devices.thisDevice")) !== null)
      ?.querySelector(".device-name")?.textContent;
    const added = within(devices)
      .getAllByRole("listitem")
      .find((item) => (item.textContent ?? "").includes(JOINER));
    expect(added).toHaveTextContent(t("security.devices.addedBy", { name: thisName ?? "" }));
  });

  it("says when the code expired, and closing mid-way cancels the session", async () => {
    const { user, requests } = renderDevices({ ...ACCOUNT, devices: { devices: 2 } });
    const devices = await openDevices(user);
    await user.click(within(devices).getByRole("button", { name: t("security.devices.add") }));
    let dialog = await screen.findByRole("dialog", { name: t("addDevice.title") });
    await user.type(within(dialog).getByRole("textbox", { name: t("addDevice.codeLabel") }), EXPIRED_CODE);
    await user.click(within(dialog).getByRole("button", { name: t("addDevice.submit") }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(/^Adding the device stopped: .+/);

    await user.type(within(dialog).getByRole("textbox", { name: t("addDevice.codeLabel") }), "ABCD EFGH JKMN");
    await user.click(within(dialog).getByRole("button", { name: t("addDevice.submit") }));
    await within(dialog).findByRole("group", { name: t("pair.digitsLabel") });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await waitFor(() => {
      expect(requests.map((request) => request.type)).toContain("PairingCancel");
    });

    // Opened again: the cancelled session is not shown, the code form is.
    await user.click(within(devices).getByRole("button", { name: t("security.devices.add") }));
    dialog = await screen.findByRole("dialog", { name: t("addDevice.title") });
    expect(within(dialog).getByRole("textbox", { name: t("addDevice.codeLabel") })).toHaveValue("");
    expect(within(dialog).queryByRole("alert")).not.toBeInTheDocument();
  });

  it("asks to connect sync first when this device has no server", async () => {
    const { user } = renderDevices({ workspaceName: "Research", security: { kitCreatedAt: NOW }, devices: { devices: 2 } });
    const devices = await openDevices(user);
    await user.click(within(devices).getByRole("button", { name: t("security.devices.add") }));
    const dialog = await screen.findByRole("dialog", { name: t("addDevice.title") });
    await user.type(within(dialog).getByRole("textbox", { name: t("addDevice.codeLabel") }), "ABCD EFGH JKMN");
    await user.click(within(dialog).getByRole("button", { name: t("addDevice.submit") }));
    expect(await within(dialog).findByRole("alert")).not.toBeEmptyDOMElement();
  });
});

describe("settings: remove a device", () => {
  it("lists who added each device and offers Remove only for the other active ones", async () => {
    const { user } = renderDevices({ ...ACCOUNT, devices: { devices: 3 } });
    const devices = await openDevices(user);
    const items = within(devices).getAllByRole("listitem");
    expect(items).toHaveLength(3);
    const mine = items.find((item) => within(item).queryByText(t("security.devices.thisDevice")) !== null);
    expect(mine).toBeDefined();
    expect(within(mine as HTMLElement).queryByRole("button", { name: /^Remove/ })).not.toBeInTheDocument();
    const others = items.filter((item) => item !== mine);
    const myName = mine?.querySelector(".device-name")?.textContent ?? "";
    for (const item of others) {
      const name = item.querySelector(".device-name")?.textContent ?? "";
      expect(within(item).getByRole("button", { name: t("security.devices.removeLabel", { name }) })).toBeEnabled();
      expect(item).toHaveTextContent(t("security.devices.addedBy", { name: myName }));
    }
  });

  it("explains the new kit, asks the passphrase with the lock on, and shows the new kit", async () => {
    const { user } = renderDevices({
      ...ACCOUNT,
      security: { kitCreatedAt: NOW, appLockPassphrase: PASSPHRASE },
      devices: { devices: 2 },
    });
    const devices = await openDevices(user);
    const other = within(devices)
      .getAllByRole("listitem")
      .find((item) => within(item).queryByText(t("security.devices.thisDevice")) === null) as HTMLElement;
    const name = other.querySelector(".device-name")?.textContent ?? "";
    await user.click(within(other).getByRole("button", { name: t("security.devices.removeLabel", { name }) }));

    const dialog = await screen.findByRole("dialog", { name: t("removeDevice.title", { name }) });
    expect(dialog).toHaveAccessibleDescription(
      "Removing a device rotates your keys and issues a new recovery kit. The old kit stops working. The removed device keeps what it already has.",
    );
    await user.click(within(dialog).getByRole("button", { name: t("removeDevice.confirm") }));
    expect(within(dialog).getByRole("alert")).toHaveTextContent(t("removeDevice.error.empty"));
    await user.type(within(dialog).getByLabelText(t("removeDevice.passphraseLabel")), "not the passphrase");
    await user.click(within(dialog).getByRole("button", { name: t("removeDevice.confirm") }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("removeDevice.error.wrong"));

    await user.type(within(dialog).getByLabelText(t("removeDevice.passphraseLabel")), PASSPHRASE);
    await user.click(within(dialog).getByRole("button", { name: t("removeDevice.confirm") }));
    expect(await screen.findByRole("dialog", { name: t("removeDevice.newKitTitle") })).toBeInTheDocument();
    expect(screen.getByRole("list", { name: t("kit.wordsLabel") })).toBeInTheDocument();

    await waitFor(() => {
      expect(other).toHaveTextContent(/Revoked /);
    });
    expect(within(other).queryByRole("button", { name: /^Remove/ })).not.toBeInTheDocument();
  });
});

describe("rotation banner", () => {
  it("asks for the passphrase to finish a rotation made elsewhere", async () => {
    const { user } = renderDevices({
      workspaceName: "Research",
      security: { kitCreatedAt: NOW, appLockPassphrase: PASSPHRASE },
      devices: { devices: 2, rotationPending: true },
    });
    const banner = await screen.findByRole("complementary", { name: t("rotationBanner.label") });
    expect(banner).toHaveTextContent("Your keys were rotated on another device. Enter your passphrase to finish.");
    await user.click(within(banner).getByRole("button", { name: t("rotationBanner.submit") }));
    expect(within(banner).getByRole("alert")).toHaveTextContent(t("rotationBanner.error.empty"));
    await user.type(within(banner).getByLabelText(t("rotationBanner.passphraseLabel")), "not the passphrase");
    await user.click(within(banner).getByRole("button", { name: t("rotationBanner.submit") }));
    expect(await within(banner).findByRole("alert")).toHaveTextContent(t("rotationBanner.error.wrong"));
    await user.type(within(banner).getByLabelText(t("rotationBanner.passphraseLabel")), PASSPHRASE);
    await user.click(within(banner).getByRole("button", { name: t("rotationBanner.submit") }));
    await waitFor(() => {
      expect(screen.queryByRole("complementary", { name: t("rotationBanner.label") })).not.toBeInTheDocument();
    });
  });

  it("shows in Settings too", async () => {
    const { user } = renderDevices({
      workspaceName: "Research",
      security: { kitCreatedAt: NOW, appLockPassphrase: PASSPHRASE },
      devices: { devices: 2, rotationPending: true },
    });
    await openScreen(user, "nav.settings");
    await screen.findByRole("heading", { name: t("settings.title") });
    expect(screen.getByRole("complementary", { name: t("rotationBanner.label") })).toBeInTheDocument();
  });

  it("is not there without a pending rotation", async () => {
    renderDevices({ ...ACCOUNT, devices: { devices: 2 } });
    await findMainNavigation();
    expect(screen.queryByRole("complementary", { name: t("rotationBanner.label") })).not.toBeInTheDocument();
    expect(screen.queryByRole("complementary", { name: t("staleKitBanner.label") })).not.toBeInTheDocument();
  });
});

describe("device notices and the removed screen", () => {
  it("shows once what another device changed, until dismissed", async () => {
    const { user, memory } = renderDevices({ ...ACCOUNT, devices: { devices: 3 } });
    const devices = await openDevices(user);
    const removed = within(devices)
      .getAllByRole("listitem")
      .find((item) => within(item).queryByText(t("security.devices.thisDevice")) === null) as HTMLElement;
    const removedName = removed.querySelector(".device-name")?.textContent ?? "";
    const listed = await new CoreClient(memory).deviceList();
    const id = listed.find((device) => device.name === removedName)?.deviceId ?? "";

    memory.receiveDeviceLog({ added: ["Tablet"], revoked: [id], rotate: true });
    const notice = await screen.findByRole("complementary", { name: t("devicesNotice.label") });
    await waitFor(() => {
      expect(notice).toHaveTextContent(t("devicesNotice.added", { name: "Tablet" }));
    });
    expect(notice).toHaveTextContent(t("devicesNotice.removed", { name: removedName }));
    expect(notice).toHaveTextContent(t("devicesNotice.rotated"));
    await user.click(within(notice).getByRole("button", { name: t("devicesNotice.dismiss") }));
    expect(screen.queryByRole("complementary", { name: t("devicesNotice.label") })).not.toBeInTheDocument();
  });

  it("a removed device shows the removed screen, with Export a backup", async () => {
    const { user, memory } = renderDevices({ ...ACCOUNT, devices: { devices: 2 } });
    await findMainNavigation();
    memory.receiveDeviceLog({ revokeThisDevice: true });
    const screenTitle = await screen.findByRole("heading", { name: t("revoked.title") });
    const card = screenTitle.closest("section") as HTMLElement;
    expect(card).toHaveTextContent("Your notes stay here; sync is off.");
    expect(screen.queryByRole("navigation", { name: t("nav.label") })).not.toBeInTheDocument();
    await user.click(within(card).getByRole("button", { name: t("revoked.export") }));
    expect(await within(card).findByRole("status")).toHaveTextContent(
      t("backupExport.saved", { path: "/backups/atomik-kb-2026-10-03.atomikkb" }),
    );
  });

  it("starts on the removed screen when the device already was", async () => {
    renderDevices({ workspaceName: "Research", security: { kitCreatedAt: NOW }, devices: { thisDeviceRevoked: true } });
    expect(await screen.findByRole("heading", { name: t("revoked.title") })).toBeInTheDocument();
  });
});
