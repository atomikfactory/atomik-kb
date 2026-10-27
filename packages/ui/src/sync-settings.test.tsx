import { CoreError, MemoryTransport } from "@atomik-kb/core-client";
import type { MemoryTransportOptions, Request, Response, SyncInfo, Transport } from "@atomik-kb/core-client";
import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t, tp } from "./i18n";
import type { MessageKey } from "./i18n";
import { describeTransfers } from "./screens/SyncSettings";
import { NOW, findMainNavigation, openScreen, recordingTransport, renderWith } from "./test/render";
import type { User } from "./test/render";
import { withTransfers } from "./test/transfers";

const SERVER = "https://sync.example.com";
const MINUTE = 60_000;

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

/** The memory core (fast fake driver unless asked otherwise) behind a recording transport. */
function memoryCore(options: MemoryTransportOptions = {}): MemoryTransport {
  return new MemoryTransport({ now: () => NOW, workspaceName: "Research", syncStepMs: 5, ...options });
}

/** Renders the app over `memory` (wrapped by `wrap`), recording requests, and waits for the main layout. */
async function renderSync(
  memory: MemoryTransport,
  wrap: (transport: Transport) => Transport = (transport) => transport,
) {
  const recording = recordingTransport(memory);
  const app = renderWith(wrap(recording.transport));
  await findMainNavigation();
  const configures = (): unknown[] =>
    recording.requests.filter((request) => request.type === "SyncConfigure").map((request) => request.data);
  return { ...app, memory, requests: recording.requests, configures };
}

/** Opens Settings and returns its Sync section. */
async function openSync(user: User): Promise<HTMLElement> {
  await openScreen(user, "nav.settings");
  return screen.findByRole("region", { name: t("settings.sync") });
}

/** The `<dd>` next to the `<dt>` with this label, inside `container`. */
function fact(container: HTMLElement, label: MessageKey): HTMLElement | null {
  const term = within(container).getByText(t(label), { selector: "dt" });
  return term.nextElementSibling as HTMLElement | null;
}

/** The indicator in the sidebar, if shown. */
function indicator(): HTMLElement | null {
  const nav = screen.getByRole("navigation", { name: t("nav.label") });
  return nav.querySelector(".sync-dot");
}

function indicatorLabel(state: MessageKey): string {
  return t("sync.indicator", { state: t(state) });
}

/**
 * A core that refuses at once (`InvalidInput` for `inviteToken`) when the server does not
 * know the account: connecting without a token, or with the token `bad`. These requests are
 * answered here and never reach the recording.
 */
function unknownAccount(transport: Transport): Transport {
  return {
    ...transport,
    dispatch: (request: Request): Promise<Response> => {
      if (request.type === "SyncConfigure" && request.data.serverUrl !== undefined) {
        const token = request.data.inviteToken;
        if (token === undefined || token === "bad") {
          return Promise.reject(
            new CoreError({
              code: "InvalidInput",
              message: token === undefined ? "an invite token is required" : "the server refused the invite token",
              details: { field: "inviteToken" },
            }),
          );
        }
      }
      return transport.dispatch(request);
    },
  };
}

/** A transport that reports other numbers in `SyncStatus` (the mirror has no other devices). */
function reporting(patch: Partial<Pick<SyncInfo, "pendingEntries" | "devicesSeen" | "transfers">>) {
  return (transport: Transport): Transport => ({
    ...transport,
    dispatch: async (request: Request): Promise<Response> => {
      const response = await transport.dispatch(request);
      return response.type === "Sync" ? { type: "Sync", data: { ...response.data, ...patch } } : response;
    },
  });
}

const UNKNOWN_ACCOUNT = "the server does not know this account; an invite token is required";
const REFUSED_TOKEN = "the server refused the invite token";

/**
 * Stands in for a driver that found the server does not know the account (the real app's
 * path): the status says `Error` and `needsInviteToken` until a configuration carries a token
 * the server takes; the token `nope` is refused (`Error`, still `needsInviteToken`).
 */
function driverWantsToken(transport: Transport): Transport {
  let wants = true;
  let lastError = UNKNOWN_ACCOUNT;
  return {
    ...transport,
    dispatch: async (request: Request): Promise<Response> => {
      if (request.type === "SyncConfigure" && request.data.inviteToken !== undefined) {
        if (request.data.inviteToken === "nope") {
          lastError = REFUSED_TOKEN;
        } else {
          wants = false;
        }
      }
      const response = await transport.dispatch(request);
      return response.type === "Sync" && wants
        ? { type: "Sync", data: { ...response.data, state: "Error", lastError, needsInviteToken: true } }
        : response;
    },
  };
}

describe("settings: sync, not connected", () => {
  it("explains sync and asks for the server address; no indicator while off", async () => {
    const { user } = await renderSync(memoryCore());
    const section = await openSync(user);
    expect(section).toHaveTextContent(t("sync.intro"));
    const field = within(section).getByRole("textbox", { name: t("sync.urlLabel") });
    expect(field).toHaveAttribute("placeholder", "https://sync.example.com");
    expect(within(section).getByRole("button", { name: t("sync.connect") })).toBeEnabled();
    expect(indicator()).toBeNull();
    expect(screen.queryByRole("complementary", { name: t("syncBanner.label") })).not.toBeInTheDocument();
  });

  it("sits between Security and Data", async () => {
    const { user } = await renderSync(memoryCore());
    await openSync(user);
    const headings = screen.getAllByRole("heading", { level: 2 }).map((heading) => heading.textContent);
    expect(headings.indexOf(t("settings.sync"))).toBe(headings.indexOf(t("settings.security")) + 1);
    expect(headings.indexOf(t("settings.data"))).toBe(headings.indexOf(t("settings.sync")) + 1);
  });

  it("explains an empty address and one the core refuses, at the field", async () => {
    const { user, configures } = await renderSync(memoryCore());
    const section = await openSync(user);
    const field = within(section).getByRole("textbox", { name: t("sync.urlLabel") });
    await user.click(within(section).getByRole("button", { name: t("sync.connect") }));
    expect(within(section).getByRole("alert")).toHaveTextContent(t("sync.error.emptyUrl"));
    expect(field).toHaveAttribute("aria-invalid", "true");

    await user.type(field, "ftp://sync.example.com");
    expect(within(section).queryByRole("alert")).not.toBeInTheDocument();
    await user.click(within(section).getByRole("button", { name: t("sync.connect") }));
    const alert = await within(section).findByRole("alert");
    expect(alert.textContent).toMatch(/^This address cannot be used: .+\.$/);
    expect(field).toHaveAccessibleDescription(alert.textContent ?? "");
    expect(configures()).toEqual([]);
  });

  it("connects, then shows the server and the status", async () => {
    vi.setSystemTime(NOW + 30_000);
    const { user, configures } = await renderSync(memoryCore());
    const section = await openSync(user);
    await user.type(within(section).getByRole("textbox", { name: t("sync.urlLabel") }), ` ${SERVER} `);
    await user.click(within(section).getByRole("button", { name: t("sync.connect") }));

    await waitFor(() => {
      expect(fact(section, "sync.server")).toHaveTextContent(SERVER);
    });
    expect(configures()).toEqual([{ serverUrl: SERVER }]);
    expect(within(section).queryByRole("textbox")).not.toBeInTheDocument();
    // The first cycle ends Idle, synced at the core's clock.
    await waitFor(() => {
      expect(fact(section, "sync.status")).toHaveTextContent(t("sync.state.Idle"));
      expect(fact(section, "sync.status")).toHaveTextContent(t("sync.lastSynced", { time: t("time.justNow") }));
    });
    expect(within(section).getByRole("button", { name: t("sync.now") })).toBeEnabled();
    expect(within(section).getByRole("button", { name: t("sync.disconnect") })).toBeEnabled();
    expect(indicator()).toHaveAttribute("title", indicatorLabel("sync.state.Idle"));
  });
});

describe("settings: sync, invite token refused at once (InvalidInput)", () => {
  it("asks for the invite token when the server does not know the account, and connects with it", async () => {
    const { user, configures } = await renderSync(memoryCore(), unknownAccount);
    const section = await openSync(user);
    await user.type(within(section).getByRole("textbox", { name: t("sync.urlLabel") }), SERVER);
    await user.click(within(section).getByRole("button", { name: t("sync.connect") }));

    const token = await within(section).findByRole("textbox", { name: t("sync.token.label") });
    expect(section).toHaveTextContent(t("sync.token.intro"));
    expect(token).toHaveAccessibleDescription(t("sync.token.intro"));
    expect(token).toHaveFocus();
    expect(fact(section, "sync.server")).toHaveTextContent(SERVER);
    expect(within(section).queryByRole("textbox", { name: t("sync.urlLabel") })).not.toBeInTheDocument();
    expect(within(section).queryByRole("alert")).not.toBeInTheDocument();

    // Nothing typed: no request.
    await user.click(within(section).getByRole("button", { name: t("sync.connect") }));
    expect(within(section).getByRole("alert")).toHaveTextContent(t("sync.token.error.empty"));
    expect(configures()).toEqual([]);

    // A token the server refuses.
    await user.type(token, "bad");
    await user.click(within(section).getByRole("button", { name: t("sync.connect") }));
    const refused = await within(section).findByRole("alert");
    expect(refused.textContent).toMatch(/^This invite token was not accepted: .+\.$/);
    expect(token).toHaveValue("");
    expect(token).toHaveAttribute("aria-invalid", "true");

    // The right one.
    await user.type(token, " invite-123 ");
    await user.click(within(section).getByRole("button", { name: t("sync.connect") }));
    await waitFor(() => {
      expect(within(section).queryByRole("textbox")).not.toBeInTheDocument();
    });
    expect(fact(section, "sync.server")).toHaveTextContent(SERVER);
    // The refused attempts were answered by `unknownAccount`, before the recording.
    expect(configures()).toEqual([{ serverUrl: SERVER, inviteToken: "invite-123" }]);
  });

  it("goes back to the address with 'Use another server'", async () => {
    const { user } = await renderSync(memoryCore(), unknownAccount);
    const section = await openSync(user);
    await user.type(within(section).getByRole("textbox", { name: t("sync.urlLabel") }), SERVER);
    await user.click(within(section).getByRole("button", { name: t("sync.connect") }));
    await user.type(await within(section).findByRole("textbox", { name: t("sync.token.label") }), "secret");
    await user.click(within(section).getByRole("button", { name: t("sync.token.changeServer") }));

    const field = within(section).getByRole("textbox", { name: t("sync.urlLabel") });
    expect(field).toHaveValue(SERVER);
    expect(within(section).queryByRole("textbox", { name: t("sync.token.label") })).not.toBeInTheDocument();
    expect(section).not.toHaveTextContent("secret");
  });
});

describe("settings: sync, the server wants an invite token (needsInviteToken)", () => {
  it("shows the server, the error and the token step instead of 'Sync now'", async () => {
    const { user } = await renderSync(memoryCore({ sync: { state: "idle" } }), driverWantsToken);
    expect(indicator()).toHaveAttribute("title", indicatorLabel("sync.state.Error"));
    const section = await openSync(user);
    expect(fact(section, "sync.server")).toHaveTextContent(SERVER);
    expect(fact(section, "sync.status")).toHaveTextContent(t("sync.state.Error"));
    expect(section).toHaveTextContent(t("sync.lastError", { message: UNKNOWN_ACCOUNT }));
    const token = within(section).getByRole("textbox", { name: t("sync.token.label") });
    expect(token).toHaveAccessibleDescription(t("sync.token.intro"));
    // Opening Settings does not jump into the field.
    expect(token).not.toHaveFocus();
    // The server shows once, read-only, in the status.
    expect(within(section).getAllByText(t("sync.server"), { selector: "dt" })).toHaveLength(1);
    expect(within(section).queryByRole("textbox", { name: t("sync.urlLabel") })).not.toBeInTheDocument();
    expect(within(section).queryByRole("button", { name: t("sync.now") })).not.toBeInTheDocument();
    expect(within(section).getByRole("button", { name: t("sync.disconnect") })).toBeEnabled();
    expect(within(section).getByRole("button", { name: t("sync.token.changeServer") })).toBeEnabled();
  });

  it("connects again with the token; a refused one leaves the step up with the reason", async () => {
    const { user, configures } = await renderSync(memoryCore({ sync: { state: "idle" } }), driverWantsToken);
    const section = await openSync(user);
    const token = within(section).getByRole("textbox", { name: t("sync.token.label") });

    await user.click(within(section).getByRole("button", { name: t("sync.connect") }));
    expect(within(section).getByRole("alert")).toHaveTextContent(t("sync.token.error.empty"));
    expect(configures()).toEqual([]);

    await user.type(token, "nope");
    await user.click(within(section).getByRole("button", { name: t("sync.connect") }));
    await waitFor(() => {
      expect(section).toHaveTextContent(t("sync.lastError", { message: REFUSED_TOKEN }));
    });
    expect(within(section).getByRole("textbox", { name: t("sync.token.label") })).toHaveValue("");

    await user.type(within(section).getByRole("textbox", { name: t("sync.token.label") }), "invite-123");
    await user.click(within(section).getByRole("button", { name: t("sync.connect") }));
    await waitFor(() => {
      expect(within(section).queryByRole("textbox")).not.toBeInTheDocument();
    });
    expect(within(section).getByRole("button", { name: t("sync.now") })).toBeInTheDocument();
    await waitFor(() => {
      expect(fact(section, "sync.status")).toHaveTextContent(t("sync.state.Idle"));
    });
    expect(configures()).toEqual([
      { serverUrl: SERVER, inviteToken: "nope" },
      { serverUrl: SERVER, inviteToken: "invite-123" },
    ]);
  });

  it("walks the mirror's unknown account: connect, the token step after the cycle, a refused token, a good one", async () => {
    const { user, configures } = await renderSync(memoryCore({ sync: { accountKnown: false } }));
    const section = await openSync(user);
    await user.type(within(section).getByRole("textbox", { name: t("sync.urlLabel") }), SERVER);
    await user.click(within(section).getByRole("button", { name: t("sync.connect") }));

    const token = await within(section).findByRole("textbox", { name: t("sync.token.label") });
    expect(fact(section, "sync.status")).toHaveTextContent(t("sync.state.Error"));
    expect(section).toHaveTextContent(t("sync.lastError", { message: UNKNOWN_ACCOUNT }));
    expect(indicator()).toHaveAttribute("title", indicatorLabel("sync.state.Error"));

    await user.type(token, "bad");
    await user.click(within(section).getByRole("button", { name: t("sync.connect") }));
    await waitFor(() => {
      expect(section).toHaveTextContent(t("sync.lastError", { message: REFUSED_TOKEN }));
    });
    const again = await within(section).findByRole("textbox", { name: t("sync.token.label") });
    expect(again).toHaveValue("");

    await user.type(again, "invite-123");
    await user.click(within(section).getByRole("button", { name: t("sync.connect") }));
    await waitFor(() => {
      expect(fact(section, "sync.status")).toHaveTextContent(t("sync.state.Idle"));
    });
    expect(within(section).queryByRole("textbox")).not.toBeInTheDocument();
    expect(section.querySelector(".sync-last-error")).toBeNull();
    expect(configures()).toEqual([
      { serverUrl: SERVER },
      { serverUrl: SERVER, inviteToken: "bad" },
      { serverUrl: SERVER, inviteToken: "invite-123" },
    ]);
  });

  it("'Use another server' edits the address, and asks for a token for the new one", async () => {
    const { user, configures } = await renderSync(memoryCore({ sync: { state: "idle" } }), driverWantsToken);
    const section = await openSync(user);
    await user.click(within(section).getByRole("button", { name: t("sync.token.changeServer") }));
    const field = within(section).getByRole("textbox", { name: t("sync.urlLabel") });
    expect(field).toHaveValue(SERVER);
    await user.clear(field);
    await user.type(field, "https://other.example.com");
    await user.click(within(section).getByRole("button", { name: t("sync.connect") }));

    await waitFor(() => {
      expect(fact(section, "sync.server")).toHaveTextContent("https://other.example.com");
    });
    expect(await within(section).findByRole("textbox", { name: t("sync.token.label") })).toBeInTheDocument();
    expect(within(section).queryByRole("textbox", { name: t("sync.urlLabel") })).not.toBeInTheDocument();
    expect(configures()).toEqual([{ serverUrl: "https://other.example.com" }]);
  });
});

describe("settings: sync, connected", () => {
  it("shows the state, when it last synced, pending changes and the devices seen", async () => {
    vi.setSystemTime(NOW + 3 * MINUTE);
    const { user } = await renderSync(
      memoryCore({ sync: { state: "idle" } }),
      reporting({ pendingEntries: 3, devicesSeen: 2 }),
    );
    const section = await openSync(user);
    const status = fact(section, "sync.status");
    expect(status).toHaveTextContent(
      [
        t("sync.state.Idle"),
        t("sync.lastSynced", { time: "3 min ago" }),
        tp("sync.pending", 3),
        tp("sync.devices", 2),
      ].join(" · "),
    );
    expect(status?.querySelector("time")).toHaveAttribute("datetime", new Date(NOW).toISOString());
    expect(fact(section, "sync.server")).toHaveTextContent(SERVER);
    expect(section.querySelector(".sync-last-error")).toBeNull();
  });

  it("uses the singular for one, and leaves the pending count out at zero", async () => {
    vi.setSystemTime(NOW + 2 * 60 * MINUTE);
    const { user } = await renderSync(memoryCore({ sync: { state: "idle" } }), reporting({ pendingEntries: 1, devicesSeen: 1 }));
    const section = await openSync(user);
    expect(fact(section, "sync.status")).toHaveTextContent(
      [t("sync.state.Idle"), t("sync.lastSynced", { time: "2 h ago" }), tp("sync.pending", 1), tp("sync.devices", 1)].join(" · "),
    );
  });

  it("without other devices or a pending change", async () => {
    const { user } = await renderSync(memoryCore({ sync: { state: "idle" } }));
    const section = await openSync(user);
    const status = fact(section, "sync.status");
    expect(status).toHaveTextContent(t("sync.devices.none"));
    expect(status).not.toHaveTextContent(/waiting to upload/);
  });

  it("shows the last error, and the state Offline", async () => {
    const { user } = await renderSync(memoryCore({ sync: { state: "offline", lastError: "connection refused" } }));
    const section = await openSync(user);
    expect(fact(section, "sync.status")).toHaveTextContent(t("sync.state.Offline"));
    expect(section).toHaveTextContent(t("sync.lastError", { message: "connection refused" }));
  });

  it("'Sync now' asks for a cycle and waits while data moves", async () => {
    const memory = memoryCore({ sync: { state: "idle" }, syncStepMs: 150 });
    const { user, requests } = await renderSync(memory);
    const section = await openSync(user);
    const button = within(section).getByRole("button", { name: t("sync.now") });
    await user.click(button);
    expect(requests.map((request) => request.type)).toContain("SyncNow");
    await waitFor(() => {
      expect(fact(section, "sync.status")).toHaveTextContent(t("sync.state.Pushing"));
    });
    expect(button).toBeDisabled();
    await waitFor(() => {
      expect(fact(section, "sync.status")).toHaveTextContent(t("sync.state.Pulling"));
    });
    expect(button).toBeDisabled();
    await waitFor(() => {
      expect(fact(section, "sync.status")).toHaveTextContent(t("sync.state.Idle"));
    });
    expect(button).toBeEnabled();
  });

  it("'Sync now' says why when the core refuses", async () => {
    const memory = memoryCore({ sync: { state: "idle" } });
    const { user } = await renderSync(memory, (transport) => ({
      ...transport,
      dispatch: (request) =>
        request.type === "SyncNow"
          ? Promise.reject(new CoreError({ code: "Storage", message: "disk I/O error" }))
          : transport.dispatch(request),
    }));
    const section = await openSync(user);
    await user.click(within(section).getByRole("button", { name: t("sync.now") }));
    expect(await within(section).findByRole("alert")).toHaveTextContent(
      t("sync.now.error", { message: "disk I/O error" }),
    );
  });

  it("disconnects after a confirmation that the notes stay, and offers to connect again", async () => {
    const { user, configures } = await renderSync(memoryCore({ sync: { state: "idle" } }));
    const section = await openSync(user);
    await user.click(within(section).getByRole("button", { name: t("sync.disconnect") }));
    let dialog = await screen.findByRole("alertdialog", { name: t("sync.disconnect.title") });
    expect(dialog).toHaveAccessibleDescription(t("sync.disconnect.body"));
    expect(dialog).toHaveTextContent("Your notes stay on this device");
    await user.click(within(dialog).getByRole("button", { name: t("dialog.cancel") }));
    expect(configures()).toEqual([]);

    await user.click(within(section).getByRole("button", { name: t("sync.disconnect") }));
    dialog = await screen.findByRole("alertdialog", { name: t("sync.disconnect.title") });
    await user.click(within(dialog).getByRole("button", { name: t("sync.disconnect.confirm") }));
    await within(section).findByRole("textbox", { name: t("sync.urlLabel") });
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(configures()).toEqual([{}]);
    expect(indicator()).toBeNull();
  });

  it("can read the status again when it could not be read", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    let failures = 1;
    const { user } = await renderSync(memoryCore({ sync: { state: "idle" } }), (transport) => ({
      ...transport,
      dispatch: (request) => {
        if (request.type === "SyncStatus" && failures > 0) {
          failures -= 1;
          return Promise.reject(new CoreError({ code: "Storage", message: "disk I/O error" }));
        }
        return transport.dispatch(request);
      },
    }));
    const section = await openSync(user);
    expect(section).toHaveTextContent(t("sync.unavailable"));
    expect(indicator()).toBeNull();
    await user.click(within(section).getByRole("button", { name: t("sync.retry") }));
    await waitFor(() => {
      expect(fact(section, "sync.server")).toHaveTextContent(SERVER);
    });
  });
});

describe("settings: sync, attachment transfers", () => {
  const transfersRow = (section: HTMLElement): HTMLElement | null =>
    within(section).queryByText(t("attachment.transfer.label"), { selector: "dt" });

  it("counts the chunks still to move, leaving out the counts at zero", async () => {
    const { user } = await renderSync(
      memoryCore({ sync: { state: "idle" } }),
      reporting({ transfers: { uploads: 3, downloads: 2, waiting: 1 } }),
    );
    const section = await openSync(user);
    await waitFor(() => {
      expect(fact(section, "attachment.transfer.label")).toHaveTextContent(
        [
          t("attachment.transfer.uploads", { count: 3 }),
          t("attachment.transfer.downloads", { count: 2 }),
          t("attachment.transfer.waitingCount", { count: 1 }),
        ].join(", "),
      );
    });
    expect(fact(section, "attachment.transfer.label")).toHaveTextContent(
      "3 to upload, 2 to download, 1 waiting for another device",
    );
  });

  it("shows no line while nothing moves", async () => {
    const { user } = await renderSync(memoryCore({ sync: { state: "idle" } }));
    const section = await openSync(user);
    await waitFor(() => {
      expect(fact(section, "sync.status")).toHaveTextContent(t("sync.state.Idle"));
    });
    expect(transfersRow(section)).toBeNull();
  });

  it("follows the chunks as they arrive (AttachmentsChanged)", async () => {
    const transfers = { uploads: 0, downloads: 2, waiting: 1 };
    const core: { events?: ReturnType<typeof withTransfers> } = {};
    const { user } = await renderSync(memoryCore({ sync: { state: "idle" } }), (transport) => {
      const wrapped: Transport = {
        ...transport,
        dispatch: async (request) => {
          const response = await transport.dispatch(request);
          return response.type === "Sync" ? { type: "Sync", data: { ...response.data, transfers: { ...transfers } } } : response;
        },
      };
      core.events = withTransfers(wrapped);
      return wrapped;
    });
    const section = await openSync(user);
    await waitFor(() => {
      expect(fact(section, "attachment.transfer.label")).toHaveTextContent("2 to download, 1 waiting for another device");
    });

    transfers.downloads = 1;
    core.events?.changed("b0000000-0000-4000-8000-000000000001");
    await waitFor(() => {
      expect(fact(section, "attachment.transfer.label")).toHaveTextContent("1 to download, 1 waiting for another device");
    });

    transfers.downloads = 0;
    transfers.waiting = 0;
    core.events?.changed("b0000000-0000-4000-8000-000000000001");
    await waitFor(() => {
      expect(transfersRow(section)).toBeNull();
    });
  });

  it("words the counts", () => {
    expect(describeTransfers({ uploads: 0, downloads: 0, waiting: 0 })).toBeNull();
    expect(describeTransfers(undefined)).toBeNull();
    expect(describeTransfers({ uploads: 1, downloads: 0, waiting: 0 })).toBe("1 to upload");
    expect(describeTransfers({ uploads: 0, downloads: 257, waiting: 0 })).toBe("257 to download");
    expect(describeTransfers({ uploads: 0, downloads: 0, waiting: 4 })).toBe("4 waiting for another device");
    expect(describeTransfers({ uploads: 2, downloads: 0, waiting: 4 })).toBe("2 to upload, 4 waiting for another device");
  });
});

describe("sync fork banner", () => {
  it("shows on Home and in Settings while the server's history does not match", async () => {
    const { user } = await renderSync(memoryCore({ sync: { state: "forked" } }));
    const banner = await screen.findByRole("complementary", { name: t("syncBanner.label") });
    expect(banner).toHaveTextContent(
      "The server's history does not match this device's. Uploads are paused so nothing is overwritten.",
    );
    expect(within(banner).getByRole("button", { name: t("syncBanner.export") })).toBeEnabled();
    expect(within(banner).getByRole("button", { name: t("syncBanner.disconnect") })).toBeEnabled();

    const section = await openSync(user);
    expect(screen.getByRole("complementary", { name: t("syncBanner.label") })).toBeInTheDocument();
    expect(fact(section, "sync.status")).toHaveTextContent(t("sync.state.Forked"));
  });

  it("is not there without a fork", async () => {
    await renderSync(memoryCore({ sync: { state: "error" } }));
    expect(screen.queryByRole("complementary", { name: t("syncBanner.label") })).not.toBeInTheDocument();
  });

  it("'Export a backup' runs the existing export", async () => {
    const memory = memoryCore({ sync: { state: "forked" }, security: { kitCreatedAt: NOW } });
    const { user } = await renderSync(memory);
    const banner = await screen.findByRole("complementary", { name: t("syncBanner.label") });
    await user.click(within(banner).getByRole("button", { name: t("syncBanner.export") }));
    const path = "/backups/atomik-kb-2026-10-03.atomikkb";
    expect(await within(banner).findByRole("status")).toHaveTextContent(t("backupExport.saved", { path }));
    expect(memory.files.has(path)).toBe(true);
  });

  it("'Export a backup' says why it cannot without a recovery kit", async () => {
    const { user } = await renderSync(memoryCore({ sync: { state: "forked" } }));
    const banner = await screen.findByRole("complementary", { name: t("syncBanner.label") });
    await user.click(within(banner).getByRole("button", { name: t("syncBanner.export") }));
    expect(await within(banner).findByRole("alert")).toHaveTextContent(t("security.backup.needsKit"));
  });

  it("'Export a backup' says so when the file dialog was cancelled", async () => {
    const memory = memoryCore({ sync: { state: "forked" }, security: { kitCreatedAt: NOW }, cancelDialogs: true });
    const { user } = await renderSync(memory);
    const banner = await screen.findByRole("complementary", { name: t("syncBanner.label") });
    await user.click(within(banner).getByRole("button", { name: t("syncBanner.export") }));
    expect(await within(banner).findByRole("status")).toHaveTextContent(t("security.backup.cancelled"));
  });

  it("offers no export where the shell cannot write backup files", async () => {
    const memory = memoryCore({ sync: { state: "forked" } });
    await renderSync(memory, (transport) => ({ ...transport, backupExport: undefined, backupImport: undefined }));
    const banner = await screen.findByRole("complementary", { name: t("syncBanner.label") });
    expect(within(banner).queryByRole("button", { name: t("syncBanner.export") })).not.toBeInTheDocument();
    expect(within(banner).getByRole("button", { name: t("syncBanner.disconnect") })).toBeInTheDocument();
  });

  it("'Disconnect' asks, then the banner goes", async () => {
    const { user, configures } = await renderSync(memoryCore({ sync: { state: "forked" } }));
    const banner = await screen.findByRole("complementary", { name: t("syncBanner.label") });
    await user.click(within(banner).getByRole("button", { name: t("syncBanner.disconnect") }));
    const dialog = await screen.findByRole("alertdialog", { name: t("sync.disconnect.title") });
    expect(dialog).toHaveTextContent("Your notes stay on this device");
    await user.click(within(dialog).getByRole("button", { name: t("sync.disconnect.confirm") }));
    await waitFor(() => {
      expect(screen.queryByRole("complementary", { name: t("syncBanner.label") })).not.toBeInTheDocument();
    });
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(configures()).toEqual([{}]);
    expect(indicator()).toBeNull();
  });
});

describe("sidebar sync indicator", () => {
  it("is grey and says so when idle; a click opens Settings", async () => {
    const { user } = await renderSync(memoryCore({ sync: { state: "idle" } }));
    const dot = screen.getByRole("button", { name: indicatorLabel("sync.state.Idle") });
    expect(dot).toHaveAttribute("title", indicatorLabel("sync.state.Idle"));
    expect(dot).toHaveClass("sync-tone-idle");
    await user.click(dot);
    expect(await screen.findByRole("heading", { name: t("settings.title") })).toBeInTheDocument();
  });

  it("is amber when offline", async () => {
    await renderSync(memoryCore({ sync: { state: "offline" } }));
    expect(screen.getByRole("button", { name: indicatorLabel("sync.state.Offline") })).toHaveClass("sync-tone-offline");
  });

  it("is red on an error", async () => {
    await renderSync(memoryCore({ sync: { state: "error" } }));
    expect(screen.getByRole("button", { name: indicatorLabel("sync.state.Error") })).toHaveClass("sync-tone-error");
  });

  it("is blue while pushing or pulling", async () => {
    const memory = memoryCore({ sync: { state: "idle" }, syncStepMs: 150 });
    await renderSync(memory);
    await memory.dispatch({ type: "SyncNow" });
    await waitFor(() => {
      expect(indicator()).toHaveAttribute("title", indicatorLabel("sync.state.Pushing"));
    });
    expect(indicator()).toHaveClass("sync-tone-busy");
    await waitFor(() => {
      expect(indicator()).toHaveAttribute("title", indicatorLabel("sync.state.Pulling"));
    });
    expect(indicator()).toHaveClass("sync-tone-busy");
    await waitFor(() => {
      expect(indicator()).toHaveAttribute("title", indicatorLabel("sync.state.Idle"));
    });
  });

  it("stays red while forked, even while the driver pulls", async () => {
    const memory = memoryCore({ sync: { state: "forked" }, syncStepMs: 150 });
    const { user } = await renderSync(memory);
    expect(indicator()).toHaveAttribute("title", indicatorLabel("sync.state.Forked"));
    const section = await openSync(user);
    await memory.dispatch({ type: "SyncNow" });
    await waitFor(() => {
      expect(fact(section, "sync.status")).toHaveTextContent(t("sync.state.Pulling"));
    });
    expect(indicator()).toHaveAttribute("title", indicatorLabel("sync.state.Forked"));
    expect(indicator()).toHaveClass("sync-tone-error");
  });
});
