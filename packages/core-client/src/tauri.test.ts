import { beforeEach, describe, expect, it, vi } from "vitest";

import { CoreError } from "./errors";
import type { AiChatInput } from "./generated/AiChatInput";
import type { AiGenerateInput } from "./generated/AiGenerateInput";
import type { AiStreamEvent } from "./generated/AiStreamEvent";
import type { Event } from "./generated/Event";
import { mediaProtocolUrl, TauriTransport } from "./tauri";

// Stand-ins for `@tauri-apps/api/core`: `invoke` is a spy, `Channel` records what the
// transport registers so tests can push events through it like the shell would.
const tauri = vi.hoisted(() => {
  class MockChannel<T> {
    static instances: MockChannel<unknown>[] = [];
    readonly onmessage: (message: T) => void;
    constructor(onmessage: (message: T) => void) {
      this.onmessage = onmessage;
      MockChannel.instances.push(this as MockChannel<unknown>);
    }
  }
  return { invoke: vi.fn<(command: string, args?: unknown) => Promise<unknown>>(), MockChannel };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke, Channel: tauri.MockChannel }));

const created: Event = { type: "WorkspaceCreated", data: { id: "22222222-2222-4222-8222-222222222222" } };
const changed: Event = { type: "SettingChanged", data: { key: "ui.theme" } };

beforeEach(() => {
  tauri.invoke.mockReset();
  tauri.invoke.mockResolvedValue(undefined);
  tauri.MockChannel.instances.length = 0;
  vi.restoreAllMocks();
});

describe("TauriTransport.dispatch", () => {
  it("invokes the dispatch command with the request and returns the response", async () => {
    tauri.invoke.mockResolvedValueOnce({ type: "Ok" });
    const transport = new TauriTransport();
    await expect(
      transport.dispatch({ type: "SettingsSet", data: { key: "ui.theme", value: "dark" } }),
    ).resolves.toEqual({ type: "Ok" });
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("dispatch", {
      request: { type: "SettingsSet", data: { key: "ui.theme", value: "dark" } },
    });
  });

  it("rejects with a CoreError carrying the serialized ApiError", async () => {
    tauri.invoke.mockRejectedValueOnce({
      code: "InvalidInput",
      message: "must not be empty",
      details: { field: "name" },
    });
    const error: unknown = await new TauriTransport()
      .dispatch({ type: "WorkspaceCreate", data: { name: " " } })
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).toApiError()).toEqual({
      code: "InvalidInput",
      message: "must not be empty",
      details: { field: "name" },
    });
  });

  it("normalises string rejections (IPC problems) into Internal errors", async () => {
    tauri.invoke.mockRejectedValueOnce("invalid args `request` for command `dispatch`");
    await expect(new TauriTransport().dispatch({ type: "AppStatus" })).rejects.toMatchObject({
      code: "Internal",
      message: "invalid args `request` for command `dispatch`",
    });
  });

  it("normalises Error rejections into Internal errors", async () => {
    tauri.invoke.mockRejectedValueOnce(new TypeError("ipc is not available"));
    await expect(new TauriTransport().dispatch({ type: "AppStatus" })).rejects.toMatchObject({
      code: "Internal",
      message: "ipc is not available",
    });
  });
});

describe("TauriTransport.coreState", () => {
  it("returns opening, ready and locked as reported by the shell", async () => {
    const transport = new TauriTransport();
    tauri.invoke.mockResolvedValueOnce("opening");
    await expect(transport.coreState()).resolves.toBe("opening");
    tauri.invoke.mockResolvedValueOnce("ready");
    await expect(transport.coreState()).resolves.toBe("ready");
    tauri.invoke.mockResolvedValueOnce("locked");
    await expect(transport.coreState()).resolves.toBe("locked");
    expect(tauri.invoke).toHaveBeenCalledWith("core_state");
  });

  it("returns the failure the shell recorded", async () => {
    const failed = { code: "KeystoreUnavailable", message: "no usable keystore" };
    tauri.invoke.mockResolvedValueOnce({ failed });
    await expect(new TauriTransport().coreState()).resolves.toEqual({ failed });
  });

  it("rejects when the shell answers with something else", async () => {
    tauri.invoke.mockResolvedValueOnce({ failed: "not an ApiError" });
    await expect(new TauriTransport().coreState()).rejects.toMatchObject({ code: "Internal" });
    tauri.invoke.mockResolvedValueOnce("starting");
    await expect(new TauriTransport().coreState()).rejects.toMatchObject({ code: "Internal" });
  });

  it("normalises invoke failures", async () => {
    tauri.invoke.mockRejectedValueOnce("command core_state not found");
    await expect(new TauriTransport().coreState()).rejects.toMatchObject({
      code: "Internal",
      message: "command core_state not found",
    });
  });
});

describe("TauriTransport.subscribe", () => {
  it("registers one channel with subscribe_events and reuses it for every listener", () => {
    const transport = new TauriTransport();
    transport.subscribe(() => undefined);
    transport.subscribe(() => undefined);
    expect(tauri.MockChannel.instances).toHaveLength(1);
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("subscribe_events", {
      onEvent: tauri.MockChannel.instances[0],
    });
  });

  it("delivers events to every listener in order until it unsubscribes", () => {
    const transport = new TauriTransport();
    const first: Event[] = [];
    const second: Event[] = [];
    const stopFirst = transport.subscribe((event) => first.push(event));
    transport.subscribe((event) => second.push(event));
    const channel = tauri.MockChannel.instances[0];

    channel?.onmessage(created);
    channel?.onmessage(changed);
    expect(first).toEqual([created, changed]);
    expect(second).toEqual([created, changed]);

    stopFirst();
    channel?.onmessage(created);
    expect(first).toEqual([created, changed]);
    expect(second).toEqual([created, changed, created]);
  });

  it("keeps delivering to other listeners when one throws", () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const transport = new TauriTransport();
    const received: Event[] = [];
    transport.subscribe(() => {
      throw new Error("listener bug");
    });
    transport.subscribe((event) => received.push(event));
    tauri.MockChannel.instances[0]?.onmessage(created);
    expect(received).toEqual([created]);
    expect(logged).toHaveBeenCalledOnce();
  });

  it("does not share a channel between transports", () => {
    new TauriTransport().subscribe(() => undefined);
    new TauriTransport().subscribe(() => undefined);
    expect(tauri.MockChannel.instances).toHaveLength(2);
  });

  it("logs a failed registration and registers again on the next subscribe", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    tauri.invoke.mockRejectedValueOnce("command subscribe_events not found");
    const transport = new TauriTransport();
    transport.subscribe(() => undefined);
    await vi.waitFor(
      () => {
        expect(logged).toHaveBeenCalledOnce();
      },
      { timeout: 4000 },
    );
    transport.subscribe(() => undefined);
    expect(tauri.MockChannel.instances).toHaveLength(2);
    expect(tauri.invoke).toHaveBeenCalledTimes(2);
  });
});

describe("TauriTransport.onShellEvent", () => {
  const PAGE_ID = "12345678-9abc-4def-8123-456789abcdef";
  const BLOCK_ID = "0a1b2c3d-4e5f-4a6b-8c7d-8e9f0a1b2c3d";
  const link = (data: unknown) => ({ type: "deepLink", data });

  it("registers one channel with shell_events_subscribe and reuses it for every listener", () => {
    const transport = new TauriTransport();
    transport.onShellEvent(() => undefined);
    transport.onShellEvent(() => undefined);
    expect(tauri.MockChannel.instances).toHaveLength(1);
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("shell_events_subscribe", {
      onEvent: tauri.MockChannel.instances[0],
    });
  });

  it("does not touch the shell until someone listens, and keeps core events and shell messages apart", () => {
    const transport = new TauriTransport();
    transport.subscribe(() => undefined);
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("subscribe_events", expect.anything());
    transport.onShellEvent(() => undefined);
    expect(tauri.invoke).toHaveBeenCalledTimes(2);
    expect(tauri.invoke).toHaveBeenLastCalledWith("shell_events_subscribe", expect.anything());
    expect(tauri.MockChannel.instances).toHaveLength(2);
  });

  it("delivers a page link and a block link to every listener until it unsubscribes", () => {
    const transport = new TauriTransport();
    const first: unknown[] = [];
    const second: unknown[] = [];
    const stopFirst = transport.onShellEvent((event) => first.push(event));
    transport.onShellEvent((event) => second.push(event));
    const channel = tauri.MockChannel.instances[0];

    channel?.onmessage(link({ pageId: PAGE_ID }));
    channel?.onmessage(link({ pageId: PAGE_ID, blockId: BLOCK_ID }));
    expect(first).toEqual([link({ pageId: PAGE_ID }), link({ pageId: PAGE_ID, blockId: BLOCK_ID })]);
    expect(second).toEqual(first);

    stopFirst();
    channel?.onmessage(link({ pageId: PAGE_ID }));
    expect(first).toHaveLength(2);
    expect(second).toHaveLength(3);
  });

  it("delivers a capture, a lock and the state of the shortcut", () => {
    const transport = new TauriTransport();
    const received: unknown[] = [];
    transport.onShellEvent((event) => received.push(event));
    const channel = tauri.MockChannel.instances[0];
    channel?.onmessage({ type: "capture" });
    channel?.onmessage({ type: "lock" });
    for (const state of ["disabled", "registered", "taken", "invalid"]) {
      channel?.onmessage({ type: "shortcut", data: { state } });
    }
    expect(received).toEqual([
      { type: "capture" },
      { type: "lock" },
      { type: "shortcut", data: { state: "disabled" } },
      { type: "shortcut", data: { state: "registered" } },
      { type: "shortcut", data: { state: "taken" } },
      { type: "shortcut", data: { state: "invalid" } },
    ]);
  });

  it("delivers a menu choice and the menu bar's visibility", () => {
    const transport = new TauriTransport();
    const received: unknown[] = [];
    transport.onShellEvent((event) => received.push(event));
    const channel = tauri.MockChannel.instances[0];
    for (const id of ["import", "export", "print", "undo", "redo", "find", "toggleSidebar", "back", "forward", "about"]) {
      channel?.onmessage({ type: "menu", data: { id } });
    }
    channel?.onmessage({ type: "menuBar", data: { visible: true } });
    channel?.onmessage({ type: "menuBar", data: { visible: false } });
    expect(received).toEqual([
      ...["import", "export", "print", "undo", "redo", "find", "toggleSidebar", "back", "forward", "about"].map((id) => ({
        type: "menu",
        data: { id },
      })),
      { type: "menuBar", data: { visible: true } },
      { type: "menuBar", data: { visible: false } },
    ]);
  });

  it("delivers an invitation link as { link } and drops one without a text", () => {
    const transport = new TauriTransport();
    const received: unknown[] = [];
    transport.onShellEvent((event) => received.push(event));
    const channel = tauri.MockChannel.instances[0];
    const LINK = "atomik-kb://invite?server=http%3A%2F%2F127.0.0.1%3A8787#8H2XG7MQ4TNP5WRZ3KVB9D6YCEJA1F0S8H2XG7MQ4TNP5WRZ3KAB";
    channel?.onmessage({ type: "inviteLink", data: { link: LINK } });
    for (const bad of [
      { type: "inviteLink" },
      { type: "inviteLink", data: null },
      { type: "inviteLink", data: {} },
      { type: "inviteLink", data: { link: "" } },
      { type: "inviteLink", data: { link: 5 } },
      { type: "inviteLink", data: LINK },
    ]) {
      channel?.onmessage(bad);
    }
    // Extra fields are not passed on.
    channel?.onmessage({ type: "inviteLink", data: { link: LINK, other: 1 } });
    expect(received).toEqual([
      { type: "inviteLink", data: { link: LINK } },
      { type: "inviteLink", data: { link: LINK } },
    ]);
  });

  it("drops a menu message of a command it does not know, and a visibility that is not a boolean", () => {
    const transport = new TauriTransport();
    const received: unknown[] = [];
    transport.onShellEvent((event) => received.push(event));
    const channel = tauri.MockChannel.instances[0];
    for (const bad of [
      { type: "menu" },
      { type: "menu", data: null },
      { type: "menu", data: {} },
      { type: "menu", data: { id: "newPage" } },
      { type: "menu", data: { id: "quit" } },
      { type: "menu", data: { id: "Print" } },
      { type: "menu", data: { id: 5 } },
      { type: "menu", data: "print" },
      { type: "menuBar" },
      { type: "menuBar", data: null },
      { type: "menuBar", data: {} },
      { type: "menuBar", data: { visible: "true" } },
      { type: "menuBar", data: { visible: 1 } },
      { type: "menuBar", data: true },
    ]) {
      channel?.onmessage(bad);
    }
    expect(received).toEqual([]);
    // Extra fields are not passed on.
    channel?.onmessage({ type: "menu", data: { id: "print", other: 1 } });
    channel?.onmessage({ type: "menuBar", data: { visible: true, other: 1 } });
    expect(received).toEqual([
      { type: "menu", data: { id: "print" } },
      { type: "menuBar", data: { visible: true } },
    ]);
  });

  it("drops what is not a message of the shape the shell sends", () => {
    const transport = new TauriTransport();
    const received: unknown[] = [];
    transport.onShellEvent((event) => received.push(event));
    const channel = tauri.MockChannel.instances[0];
    for (const bad of [
      null,
      "capture",
      5,
      [],
      {},
      { type: 5 },
      { type: "unknown" },
      { type: "deepLink" },
      { type: "deepLink", data: null },
      link({}),
      link({ pageId: "" }),
      link({ pageId: 5 }),
      link({ pageId: PAGE_ID, blockId: 5 }),
      link({ blockId: BLOCK_ID }),
      // A bare link, the shape of before the channel carried more than links.
      { pageId: PAGE_ID },
      { type: "shortcut" },
      { type: "shortcut", data: null },
      { type: "shortcut", data: {} },
      { type: "shortcut", data: { state: "other" } },
      { type: "shortcut", data: { state: 5 } },
    ]) {
      channel?.onmessage(bad);
    }
    expect(received).toEqual([]);
    // Extra fields are not passed on.
    channel?.onmessage({ type: "deepLink", data: { pageId: PAGE_ID, other: "x" }, more: 1 });
    channel?.onmessage({ type: "capture", data: { page: "x" } });
    channel?.onmessage({ type: "shortcut", data: { state: "taken", other: 1 } });
    expect(received).toEqual([link({ pageId: PAGE_ID }), { type: "capture" }, { type: "shortcut", data: { state: "taken" } }]);
  });

  it("keeps delivering to other listeners when one throws", () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const transport = new TauriTransport();
    const received: unknown[] = [];
    transport.onShellEvent(() => {
      throw new Error("listener bug");
    });
    transport.onShellEvent((event) => received.push(event));
    tauri.MockChannel.instances[0]?.onmessage({ type: "capture" });
    expect(received).toEqual([{ type: "capture" }]);
    expect(logged).toHaveBeenCalledOnce();
  });

  it("logs a failed registration without the link and registers again on the next listener", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    tauri.invoke.mockRejectedValueOnce("command shell_events_subscribe not found");
    const transport = new TauriTransport();
    transport.onShellEvent(() => undefined);
    await vi.waitFor(() => {
      expect(logged).toHaveBeenCalledOnce();
    });
    transport.onShellEvent(() => undefined);
    expect(tauri.MockChannel.instances).toHaveLength(2);
    expect(tauri.invoke).toHaveBeenCalledTimes(2);
  });
});

describe("TauriTransport.menuBarShow and menuBarHide", () => {
  it("call the two shell commands with no arguments", async () => {
    const transport = new TauriTransport();
    await transport.menuBarShow();
    expect(tauri.invoke).toHaveBeenLastCalledWith("menu_bar_show");
    await transport.menuBarHide();
    expect(tauri.invoke).toHaveBeenLastCalledWith("menu_bar_hide");
    expect(tauri.invoke).toHaveBeenCalledTimes(2);
  });

  it("reject with a CoreError when the shell does", async () => {
    tauri.invoke.mockRejectedValueOnce({ code: "Internal", message: "the shell has no menu bar" });
    const error: unknown = await new TauriTransport().menuBarShow().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).code).toBe("Internal");
    tauri.invoke.mockRejectedValueOnce("command menu_bar_hide not found");
    const other: unknown = await new TauriTransport().menuBarHide().catch((e: unknown) => e);
    expect(other).toBeInstanceOf(CoreError);
  });
});

describe("TauriTransport.shellLabelsSet", () => {
  it("sends the labels under the name the shell command takes", async () => {
    await new TauriTransport().shellLabelsSet({ open: "Open", newPage: "New page", lock: "Lock", quit: "Quit" });
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("shell_labels_set", {
      labels: { open: "Open", newPage: "New page", lock: "Lock", quit: "Quit" },
    });
  });

  it("sends the menu bar's labels under their own ids", async () => {
    await new TauriTransport().shellLabelsSet({ menuFile: "&Fichier", menuCloseWindow: "&Fermer la fenêtre" });
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("shell_labels_set", {
      labels: { menuFile: "&Fichier", menuCloseWindow: "&Fermer la fenêtre" },
    });
  });

  it("sends only the labels it is given", async () => {
    await new TauriTransport().shellLabelsSet({ quit: "Salir" });
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("shell_labels_set", { labels: { quit: "Salir" } });
  });

  it("rejects with a CoreError for the field the shell names", async () => {
    tauri.invoke.mockRejectedValueOnce({
      code: "InvalidInput",
      message: "a label is 1 to 64 characters",
      details: { field: "open" },
    });
    const error: unknown = await new TauriTransport().shellLabelsSet({ open: "" }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).code).toBe("InvalidInput");
    expect((error as CoreError).details).toEqual({ field: "open" });
  });
});

const PAGE = "33333333-3333-4333-8333-333333333333";
const pageHeader = { headers: { "x-kb-page": PAGE } };

/** The body `invoke` received in call `index`, as bytes. */
function bodyOf(index: number): Uint8Array {
  const body = tauri.invoke.mock.calls[index]?.[1];
  expect(body).toBeInstanceOf(Uint8Array);
  return body as Uint8Array;
}

describe("TauriTransport.docs", () => {
  it("open sends an empty raw body with the page header and returns the snapshot bytes", async () => {
    tauri.invoke.mockResolvedValueOnce(new Uint8Array([1, 2, 3]).buffer);
    const snapshot = await new TauriTransport().docs.open(PAGE);
    expect(snapshot).toBeInstanceOf(Uint8Array);
    expect(Array.from(snapshot)).toEqual([1, 2, 3]);
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("doc_open", expect.any(Uint8Array), pageHeader);
    expect(bodyOf(0)).toHaveLength(0);
  });

  it("import sends the update bytes as the raw body and resolves with nothing", async () => {
    const update = new Uint8Array([7, 8, 9]);
    tauri.invoke.mockResolvedValueOnce(new ArrayBuffer(0));
    await expect(new TauriTransport().docs.import(PAGE, update)).resolves.toBeUndefined();
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("doc_import", update, pageHeader);
  });

  it("export sends the encoded version vector, or an empty body for everything", async () => {
    const transport = new TauriTransport();
    const since = new Uint8Array([1, 4, 2]);
    tauri.invoke.mockResolvedValueOnce(new Uint8Array([5, 6]).buffer);
    await expect(transport.docs.export(PAGE, since)).resolves.toEqual(new Uint8Array([5, 6]));
    expect(tauri.invoke).toHaveBeenLastCalledWith("doc_export", since, pageHeader);

    tauri.invoke.mockResolvedValueOnce(new ArrayBuffer(0));
    await expect(transport.docs.export(PAGE)).resolves.toEqual(new Uint8Array());
    expect(tauri.invoke).toHaveBeenLastCalledWith("doc_export", expect.any(Uint8Array), pageHeader);
    expect(bodyOf(1)).toHaveLength(0);
  });

  it("close sends an empty raw body with the page header", async () => {
    await expect(new TauriTransport().docs.close(PAGE)).resolves.toBeUndefined();
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("doc_close", expect.any(Uint8Array), pageHeader);
    expect(bodyOf(0)).toHaveLength(0);
  });

  it("normalises failures like dispatch", async () => {
    const docs = new TauriTransport().docs;
    tauri.invoke.mockRejectedValueOnce({ code: "NotFound", message: "page not found" });
    const error: unknown = await docs.open(PAGE).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).toApiError()).toEqual({ code: "NotFound", message: "page not found" });

    tauri.invoke.mockRejectedValueOnce("missing header x-kb-page");
    await expect(docs.import(PAGE, new Uint8Array([1]))).rejects.toMatchObject({
      code: "Internal",
      message: "missing header x-kb-page",
    });
    tauri.invoke.mockRejectedValueOnce(new TypeError("ipc is not available"));
    await expect(docs.export(PAGE)).rejects.toMatchObject({ code: "Internal", message: "ipc is not available" });
    tauri.invoke.mockRejectedValueOnce({ code: "InvalidInput", message: "bad", details: { field: "since" } });
    await expect(docs.close(PAGE)).rejects.toMatchObject({ code: "InvalidInput", details: { field: "since" } });
  });

  it("accepts the byte array of Tauri's postMessage fallback", async () => {
    const docs = new TauriTransport().docs;
    tauri.invoke.mockResolvedValueOnce([1, 2, 255]);
    await expect(docs.open(PAGE)).resolves.toEqual(new Uint8Array([1, 2, 255]));
    tauri.invoke.mockResolvedValueOnce([]);
    await expect(docs.export(PAGE)).resolves.toEqual(new Uint8Array());
  });

  it("refuses results that are not bytes", async () => {
    const docs = new TauriTransport().docs;
    for (const wrong of [[1, 256], [0.5], ["1"], null, "AQID", { 0: 1 }]) {
      tauri.invoke.mockResolvedValueOnce(wrong);
      await expect(docs.open(PAGE)).rejects.toMatchObject({
        code: "Internal",
        message: "The desktop shell answered doc_open without raw bytes",
      });
    }
    tauri.invoke.mockResolvedValueOnce(undefined);
    await expect(docs.export(PAGE)).rejects.toMatchObject({ code: "Internal" });
  });

  it("works when the methods are called detached from the object", async () => {
    tauri.invoke.mockResolvedValueOnce(new ArrayBuffer(1));
    // eslint-disable-next-line @typescript-eslint/unbound-method -- detaching is exactly what this test checks
    const { open } = new TauriTransport().docs;
    await expect(open(PAGE)).resolves.toHaveLength(1);
  });
});

describe("TauriTransport.openExternal", () => {
  it("invokes open_external with the url", async () => {
    await expect(new TauriTransport().openExternal("https://example.com/a?b=c")).resolves.toBeUndefined();
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("open_external", { url: "https://example.com/a?b=c" });
  });

  it("normalises the shell's refusal", async () => {
    tauri.invoke.mockRejectedValueOnce({
      code: "InvalidInput",
      message: "unsupported scheme",
      details: { field: "url" },
    });
    await expect(new TauriTransport().openExternal("javascript:alert(1)")).rejects.toMatchObject({
      code: "InvalidInput",
      details: { field: "url" },
    });
  });
});

describe("TauriTransport app lock", () => {
  it("lock invokes lock without arguments", async () => {
    await expect(new TauriTransport().lock()).resolves.toBeUndefined();
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("lock");
  });

  it("unlock invokes unlock with the passphrase", async () => {
    await expect(new TauriTransport().unlock("correct horse")).resolves.toBeUndefined();
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("unlock", { passphrase: "correct horse" });
  });

  it("normalises the shell's refusals", async () => {
    const transport = new TauriTransport();
    tauri.invoke.mockRejectedValueOnce({ code: "InvalidInput", message: "the app lock is not enabled" });
    await expect(transport.lock()).rejects.toMatchObject({ code: "InvalidInput" });
    tauri.invoke.mockRejectedValueOnce({
      code: "InvalidInput",
      message: "wrong passphrase",
      details: { field: "passphrase" },
    });
    const error: unknown = await transport.unlock("nope nope").catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).toApiError()).toEqual({
      code: "InvalidInput",
      message: "wrong passphrase",
      details: { field: "passphrase" },
    });
    tauri.invoke.mockRejectedValueOnce("command unlock not found");
    await expect(transport.unlock("12345678")).rejects.toMatchObject({
      code: "Internal",
      message: "command unlock not found",
    });
  });

  it("reports Locked from other commands as such", async () => {
    tauri.invoke.mockRejectedValueOnce({ code: "Locked", message: "the workspace is locked" });
    await expect(new TauriTransport().dispatch({ type: "AppStatus" })).rejects.toMatchObject({
      code: "Locked",
      message: "the workspace is locked",
    });
  });
});

const summary = {
  bytes: 4096,
  documents: 7,
  settings: 2,
  createdAt: 1_700_000_000_000,
  accountId: "44444444-4444-4444-8444-444444444444",
  chunks: 3,
  chunkBytes: 9_437_304,
};

describe("TauriTransport backups", () => {
  it("backupExport invokes backup_export with the path, or without one for the save dialog", async () => {
    const transport = new TauriTransport();
    tauri.invoke.mockResolvedValueOnce({ summary, path: "C:/backups/a.atomikkb" });
    await expect(transport.backupExport("C:/backups/a.atomikkb")).resolves.toEqual({
      summary,
      path: "C:/backups/a.atomikkb",
    });
    expect(tauri.invoke).toHaveBeenLastCalledWith("backup_export", { path: "C:/backups/a.atomikkb" });

    tauri.invoke.mockResolvedValueOnce({ summary, path: "D:/picked.atomikkb" });
    await expect(transport.backupExport()).resolves.toEqual({ summary, path: "D:/picked.atomikkb" });
    expect(tauri.invoke).toHaveBeenLastCalledWith("backup_export", { path: undefined });
    // Undefined members vanish in the IPC's JSON, so the shell sees no path at all.
    expect(JSON.stringify(tauri.invoke.mock.lastCall?.[1])).toBe("{}");
  });

  it("backupImport invokes backup_import with the path and the kit, each optional", async () => {
    const transport = new TauriTransport();
    tauri.invoke.mockResolvedValue({ summary, path: "/home/me/a.atomikkb" });
    await expect(transport.backupImport("/home/me/a.atomikkb", "abandon ability")).resolves.toEqual({
      summary,
      path: "/home/me/a.atomikkb",
    });
    expect(tauri.invoke).toHaveBeenLastCalledWith("backup_import", {
      path: "/home/me/a.atomikkb",
      kit: "abandon ability",
    });
    await transport.backupImport();
    expect(tauri.invoke).toHaveBeenLastCalledWith("backup_import", { path: undefined, kit: undefined });
    await transport.backupImport(undefined, "AAAA-BBBB");
    expect(tauri.invoke).toHaveBeenLastCalledWith("backup_import", { path: undefined, kit: "AAAA-BBBB" });
  });

  it("returns only the summary and the path", async () => {
    tauri.invoke.mockResolvedValueOnce({ summary, path: "a.atomikkb", extra: true });
    await expect(new TauriTransport().backupExport("a.atomikkb")).resolves.toStrictEqual({
      summary,
      path: "a.atomikkb",
    });
  });

  it("refuses results that are not a summary and a path", async () => {
    const transport = new TauriTransport();
    const wrong = [
      undefined,
      null,
      "a.atomikkb",
      { summary },
      { path: "a.atomikkb" },
      { summary, path: 42 },
      { summary: { ...summary, bytes: "4096" }, path: "a.atomikkb" },
      { summary: { ...summary, accountId: undefined }, path: "a.atomikkb" },
      { summary: { ...summary, chunks: undefined }, path: "a.atomikkb" },
      { summary: { ...summary, chunkBytes: "0" }, path: "a.atomikkb" },
    ];
    for (const result of wrong) {
      tauri.invoke.mockResolvedValueOnce(result);
      await expect(transport.backupExport("a.atomikkb"), JSON.stringify(result)).rejects.toMatchObject({
        code: "Internal",
        message: "The desktop shell answered backup_export without a backup summary and path",
      });
    }
    tauri.invoke.mockResolvedValueOnce({ summary });
    await expect(transport.backupImport()).rejects.toMatchObject({
      code: "Internal",
      message: "The desktop shell answered backup_import without a backup summary and path",
    });
  });

  it("normalises failures: a cancelled dialog, a wrong kit, a damaged file", async () => {
    const transport = new TauriTransport();
    tauri.invoke.mockRejectedValueOnce({ code: "InvalidInput", message: "cancelled", details: { field: "path" } });
    await expect(transport.backupExport()).rejects.toMatchObject({ code: "InvalidInput", details: { field: "path" } });
    tauri.invoke.mockRejectedValueOnce({ code: "InvalidInput", message: "no match", details: { field: "kit" } });
    await expect(transport.backupImport(undefined, "x")).rejects.toMatchObject({ details: { field: "kit" } });
    tauri.invoke.mockRejectedValueOnce({ code: "Crypto", message: "authentication failed" });
    const error: unknown = await transport.backupImport("a.atomikkb").catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect(error).toMatchObject({ code: "Crypto" });
  });
});

const SPACE = "22222222-2222-4222-8222-222222222222";
const ATTACHMENT = "55555555-5555-4555-8555-555555555555";
const attachmentHeader = { headers: { "x-kb-attachment": ATTACHMENT } };
const attachmentInfo = {
  attachmentId: ATTACHMENT,
  spaceId: SPACE,
  name: "photo.png",
  mime: "image/png",
  size: 5_000_000,
  chunkCount: 2,
  createdAt: 1_700_000_000_000,
  references: 1,
  localChunks: 2,
  availability: "Complete",
};

describe("the media protocol URL (ADR-0032)", () => {
  const id = "12345678-9abc-4def-8123-456789abcdef";

  it("is http://kbmedia.localhost/<id> on Windows and Android webviews and kbmedia://localhost/<id> elsewhere", () => {
    expect(mediaProtocolUrl(id, true)).toBe(`http://kbmedia.localhost/${id}`);
    expect(mediaProtocolUrl(id, false)).toBe(`kbmedia://localhost/${id}`);
  });

  it("is null for text that is not an attachment id, so nothing else can be put in the path", () => {
    for (const text of ["", "x", `${id}/..`, `${id}?a=1`, `../${id}`, `${id}#x`, " " + id]) {
      expect(mediaProtocolUrl(text, true), text).toBeNull();
      expect(mediaProtocolUrl(text, false), text).toBeNull();
    }
  });

  it("is what the transport offers, by the webview's own platform", () => {
    const agent = vi.spyOn(navigator, "userAgent", "get");
    agent.mockReturnValue("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Edg/130");
    expect(new TauriTransport().attachments.mediaUrl?.(id)).toBe(`http://kbmedia.localhost/${id}`);
    agent.mockReturnValue("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15");
    expect(new TauriTransport().attachments.mediaUrl?.(id)).toBe(`kbmedia://localhost/${id}`);
    agent.mockReturnValue("Mozilla/5.0 (Linux; Android 14)");
    expect(new TauriTransport().attachments.mediaUrl?.(id)).toBe(`http://kbmedia.localhost/${id}`);
  });
});

describe("TauriTransport.attachments", () => {
  it("write sends the bytes as the raw body with the attachment header", async () => {
    const bytes = new Uint8Array([1, 2, 3]);
    tauri.invoke.mockResolvedValueOnce(null);
    await expect(new TauriTransport().attachments.write(ATTACHMENT, bytes)).resolves.toBeUndefined();
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("attachment_write", bytes, attachmentHeader);
  });

  it("read sends an empty body, the range as decimal headers when given, and returns the raw bytes", async () => {
    const attachments = new TauriTransport().attachments;
    tauri.invoke.mockResolvedValueOnce(new Uint8Array([7, 8]).buffer);
    await expect(attachments.read(ATTACHMENT)).resolves.toEqual(new Uint8Array([7, 8]));
    expect(tauri.invoke).toHaveBeenLastCalledWith("attachment_read", expect.any(Uint8Array), attachmentHeader);
    expect(bodyOf(0)).toHaveLength(0);

    tauri.invoke.mockResolvedValueOnce([9]);
    await expect(attachments.read(ATTACHMENT, { start: 4_194_300, end: 4_194_310 })).resolves.toEqual(
      new Uint8Array([9]),
    );
    expect(tauri.invoke).toHaveBeenLastCalledWith("attachment_read", expect.any(Uint8Array), {
      headers: { "x-kb-attachment": ATTACHMENT, "x-kb-range-start": "4194300", "x-kb-range-end": "4194310" },
    });

    tauri.invoke.mockResolvedValueOnce("not bytes");
    await expect(attachments.read(ATTACHMENT)).rejects.toMatchObject({
      code: "Internal",
      message: "The desktop shell answered attachment_read without raw bytes",
    });
  });

  it("importPath invokes attachment_import_path with the space and the path, leaving out a missing path", async () => {
    const attachments = new TauriTransport().attachments;
    tauri.invoke.mockResolvedValue(attachmentInfo);
    await expect(attachments.importPath(SPACE, "C:/in/photo.png")).resolves.toEqual(attachmentInfo);
    expect(tauri.invoke).toHaveBeenLastCalledWith("attachment_import_path", { spaceId: SPACE, path: "C:/in/photo.png" });
    await attachments.importPath(SPACE);
    expect(tauri.invoke).toHaveBeenLastCalledWith("attachment_import_path", { spaceId: SPACE });
    expect(Object.keys(tauri.invoke.mock.lastCall?.[1] as object)).toEqual(["spaceId"]);
  });

  it("importUrl invokes attachment_import_url with the space and the link, once, and returns the attachment", async () => {
    const attachments = new TauriTransport().attachments;
    tauri.invoke.mockResolvedValue(attachmentInfo);
    const link = "https://example.com/photos/sunset.png";
    await expect(attachments.importUrl?.(SPACE, link)).resolves.toEqual(attachmentInfo);
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("attachment_import_url", { spaceId: SPACE, url: link });
  });

  it("importUrl validates the answer like importPath, naming its own command", async () => {
    const attachments = new TauriTransport().attachments;
    tauri.invoke.mockResolvedValueOnce({ ...attachmentInfo, unreferencedSince: null, extra: true });
    await expect(attachments.importUrl?.(SPACE, "https://example.com/a.png")).resolves.toStrictEqual(attachmentInfo);
    for (const wrong of [null, "photo.png", { ...attachmentInfo, size: "5" }, { ...attachmentInfo, name: undefined }]) {
      tauri.invoke.mockResolvedValueOnce(wrong);
      await expect(attachments.importUrl?.(SPACE, "https://example.com/a.png"), JSON.stringify(wrong)).rejects.toMatchObject({
        code: "Internal",
        message: "The desktop shell answered attachment_import_url without an attachment",
      });
    }
  });

  it("importUrl keeps the shell's reason and field in the error, and never adds the link", async () => {
    const attachments = new TauriTransport().attachments;
    const refusal = {
      code: "InvalidInput",
      message: "the server answered with an error (HTTP 404)",
      details: { field: "url", reason: "status", status: 404 },
    };
    tauri.invoke.mockRejectedValueOnce(refusal);
    const link = "https://secret-host.example/token-abc/photo.png";
    const error: unknown = await attachments.importUrl?.(SPACE, link).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).toApiError()).toEqual(refusal);
    expect(JSON.stringify(error)).not.toContain("secret-host");
  });

  it("saveAs invokes attachment_save_as and returns the attachment and the path", async () => {
    const attachments = new TauriTransport().attachments;
    tauri.invoke.mockResolvedValue({ info: attachmentInfo, path: "D:/out/photo.png", extra: 1 });
    await expect(attachments.saveAs(ATTACHMENT, "D:/out/photo.png")).resolves.toStrictEqual({
      info: attachmentInfo,
      path: "D:/out/photo.png",
    });
    expect(tauri.invoke).toHaveBeenLastCalledWith("attachment_save_as", {
      attachmentId: ATTACHMENT,
      path: "D:/out/photo.png",
    });
    await attachments.saveAs(ATTACHMENT);
    expect(tauri.invoke).toHaveBeenLastCalledWith("attachment_save_as", { attachmentId: ATTACHMENT });
  });

  it("open invokes attachment_open and returns the path of the copy", async () => {
    tauri.invoke.mockResolvedValueOnce("C:/cache/open/x/photo.png");
    await expect(new TauriTransport().attachments.open(ATTACHMENT)).resolves.toBe("C:/cache/open/x/photo.png");
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("attachment_open", { attachmentId: ATTACHMENT });
  });

  it("validates the JSON answers: an attachment (unreferencedSince optional), a path", async () => {
    const attachments = new TauriTransport().attachments;
    tauri.invoke.mockResolvedValueOnce({ ...attachmentInfo, unreferencedSince: null, extra: true });
    await expect(attachments.importPath(SPACE)).resolves.toStrictEqual(attachmentInfo);
    tauri.invoke.mockResolvedValueOnce({ ...attachmentInfo, unreferencedSince: 5 });
    await expect(attachments.importPath(SPACE)).resolves.toEqual({ ...attachmentInfo, unreferencedSince: 5 });

    for (const wrong of [
      null,
      "photo.png",
      { ...attachmentInfo, size: "5" },
      { ...attachmentInfo, chunkCount: undefined },
      { ...attachmentInfo, unreferencedSince: "yesterday" },
    ]) {
      tauri.invoke.mockResolvedValueOnce(wrong);
      await expect(attachments.importPath(SPACE), JSON.stringify(wrong)).rejects.toMatchObject({
        code: "Internal",
        message: "The desktop shell answered attachment_import_path without an attachment",
      });
    }
    for (const wrong of [{ info: attachmentInfo }, { info: {}, path: "a" }, { info: attachmentInfo, path: 1 }]) {
      tauri.invoke.mockResolvedValueOnce(wrong);
      await expect(attachments.saveAs(ATTACHMENT)).rejects.toMatchObject({
        code: "Internal",
        message: "The desktop shell answered attachment_save_as without an attachment and path",
      });
    }
    tauri.invoke.mockResolvedValueOnce(null);
    await expect(attachments.open(ATTACHMENT)).rejects.toMatchObject({
      code: "Internal",
      message: "The desktop shell answered attachment_open without a path",
    });
  });

  it("normalises failures like dispatch", async () => {
    const attachments = new TauriTransport().attachments;
    tauri.invoke.mockRejectedValueOnce({
      code: "InvalidInput",
      message: "must be at most 2147483648 bytes",
      details: { field: "size", limit: 2_147_483_648 },
    });
    const error: unknown = await attachments.write(ATTACHMENT, new Uint8Array(1)).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).details).toEqual({ field: "size", limit: 2_147_483_648 });
    tauri.invoke.mockRejectedValueOnce({ code: "Locked", message: "the workspace is locked" });
    await expect(attachments.read(ATTACHMENT)).rejects.toMatchObject({ code: "Locked" });
    tauri.invoke.mockRejectedValueOnce({ code: "InvalidInput", message: "no file was chosen", details: { field: "path" } });
    await expect(attachments.importPath(SPACE)).rejects.toMatchObject({ details: { field: "path" } });
    tauri.invoke.mockRejectedValueOnce("missing header x-kb-attachment");
    await expect(attachments.saveAs(ATTACHMENT)).rejects.toMatchObject({
      code: "Internal",
      message: "missing header x-kb-attachment",
    });
    tauri.invoke.mockRejectedValueOnce({ code: "NotFound", message: "attachment not found" });
    await expect(attachments.open(ATTACHMENT)).rejects.toMatchObject({ code: "NotFound" });
  });

  it("works when the methods are called detached from the object", async () => {
    tauri.invoke.mockResolvedValueOnce(new ArrayBuffer(2));
    // eslint-disable-next-line @typescript-eslint/unbound-method -- detaching is exactly what this test checks
    const { read } = new TauriTransport().attachments;
    await expect(read(ATTACHMENT)).resolves.toHaveLength(2);
  });
});

// --- local AI and MCP (ADR-0037) ---

const aiStatusRaw = {
  endpoint: "http://127.0.0.1:11434",
  local: true,
  remoteConsent: false,
  reachable: true,
  version: "0.35.0",
  models: [
    { name: "qwen3:8b", family: "qwen3", parameterSize: "8.2B", capabilities: ["completion", "thinking"], size: 5_200_000_000 },
    { name: "nomic-embed-text:latest", capabilities: ["embedding"], size: 274_000_000 },
  ],
  index: { model: "nomic-embed-text:latest", pagesIndexed: 3, pagesTotal: 5, chunks: 9, dirty: 2, dimensions: 768 },
  indexer: { phase: "indexing", lastRunAt: 1_700_000_000_000 },
};

const sourceRaw = {
  n: 1,
  pageId: "66666666-6666-4666-8666-666666666666",
  spaceId: SPACE,
  blockId: "77777777-7777-4777-8777-777777777777",
  title: "Launch plan",
  icon: "🚀",
  text: "The launch is on Friday.",
  score: 0.5,
};

/** Makes the mocked `invoke` of a stream command push these messages into the channel it was given, then resolve. */
function streaming(messages: unknown[], after?: () => void): void {
  tauri.invoke.mockImplementationOnce((_command: string, args?: unknown) => {
    const channel = (args as { onEvent: InstanceType<typeof tauri.MockChannel<unknown>> }).onEvent;
    for (const message of messages) {
      channel.onmessage(message);
    }
    after?.();
    return Promise.resolve(undefined);
  });
}

describe("TauriTransport.ai", () => {
  it("status and endpointSet resolve with a validated AiStatus, leaving out what is absent", async () => {
    const ai = new TauriTransport().ai;
    tauri.invoke.mockResolvedValueOnce({ ...aiStatusRaw, extra: true });
    const status = await ai.status();
    expect(tauri.invoke).toHaveBeenLastCalledWith("ai_status");
    expect(status).toStrictEqual(aiStatusRaw);

    tauri.invoke.mockResolvedValueOnce({
      endpoint: "https://ollama.example.com",
      local: false,
      remoteConsent: true,
      reachable: false,
      version: null,
      models: [],
      error: { code: "InvalidInput", message: "Ollama could not be reached", details: { field: "endpoint", reason: "unreachable" } },
      index: null,
      indexer: { phase: "error", error: "Ollama could not be reached" },
    });
    await expect(ai.endpointSet("https://ollama.example.com/")).resolves.toStrictEqual({
      endpoint: "https://ollama.example.com",
      local: false,
      remoteConsent: true,
      reachable: false,
      models: [],
      error: { code: "InvalidInput", message: "Ollama could not be reached", details: { field: "endpoint", reason: "unreachable" } },
      indexer: { phase: "error", error: "Ollama could not be reached" },
    });
    expect(tauri.invoke).toHaveBeenLastCalledWith("ai_endpoint_set", { url: "https://ollama.example.com/" });
  });

  it("refuses a status that is not one, naming its command", async () => {
    const ai = new TauriTransport().ai;
    const wrong = [
      undefined,
      null,
      "ok",
      { ...aiStatusRaw, endpoint: 1 },
      { ...aiStatusRaw, reachable: "yes" },
      { ...aiStatusRaw, models: "none" },
      { ...aiStatusRaw, models: [{ name: "x", capabilities: "completion", size: 1 }] },
      { ...aiStatusRaw, models: [{ name: "x", capabilities: [], size: "big" }] },
      { ...aiStatusRaw, indexer: { phase: "paused" } },
      { ...aiStatusRaw, indexer: undefined },
      { ...aiStatusRaw, index: { pagesIndexed: 1 } },
      { ...aiStatusRaw, error: "no" },
    ];
    for (const result of wrong) {
      tauri.invoke.mockResolvedValueOnce(result);
      await expect(ai.status(), JSON.stringify(result)).rejects.toMatchObject({
        code: "Internal",
        message: "The desktop shell answered ai_status without an AI status",
      });
    }
    tauri.invoke.mockResolvedValueOnce({});
    await expect(ai.endpointSet("http://127.0.0.1:11434")).rejects.toMatchObject({
      message: "The desktop shell answered ai_endpoint_set without an AI status",
    });
  });

  it("keeps the shell's reason when a remote endpoint is declined, and never adds the URL", async () => {
    const refusal = {
      code: "InvalidInput",
      message: "the endpoint was not changed",
      details: { field: "url", reason: "declined" },
    };
    tauri.invoke.mockRejectedValueOnce(refusal);
    const error: unknown = await new TauriTransport().ai.endpointSet("https://secret-host.example").catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).toApiError()).toEqual(refusal);
    expect(JSON.stringify(error)).not.toContain("secret-host");
  });

  it("generate opens one channel, invokes ai_generate with the request id and the input, and delivers every event in order", async () => {
    const input: AiGenerateInput = { action: "improve", text: "Bad text" };
    const seen: AiStreamEvent[] = [];
    streaming([
      { type: "delta", text: "Better " },
      { type: "delta", text: "text" },
      { type: "done", text: "Better text", model: "qwen3:8b" },
    ]);
    await expect(new TauriTransport().ai.generate("req-1", input, (event) => seen.push(event))).resolves.toBeUndefined();
    expect(seen).toEqual([
      { type: "delta", text: "Better " },
      { type: "delta", text: "text" },
      { type: "done", text: "Better text", model: "qwen3:8b" },
    ]);
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("ai_generate", {
      requestId: "req-1",
      input,
      onEvent: tauri.MockChannel.instances[0],
    });
    expect(tauri.MockChannel.instances).toHaveLength(1);
  });

  it("chat delivers the sources first and then the answer", async () => {
    const input: AiChatInput = { messages: [{ role: "user", content: "When?" }], scope: { type: "space", spaceId: SPACE } };
    const seen: AiStreamEvent[] = [];
    streaming([
      { type: "sources", sources: [sourceRaw] },
      { type: "delta", text: "Friday [1]." },
      { type: "done", text: "Friday [1].", model: "qwen3:8b" },
    ]);
    await new TauriTransport().ai.chat("chat-1", input, (event) => seen.push(event));
    expect(seen.map((event) => event.type)).toEqual(["sources", "delta", "done"]);
    expect(seen[0]).toEqual({ type: "sources", sources: [sourceRaw] });
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("ai_chat", {
      requestId: "chat-1",
      input,
      onEvent: tauri.MockChannel.instances[0],
    });
  });

  it("a stream that fails ends with its error event, and the command still resolves", async () => {
    const seen: AiStreamEvent[] = [];
    const cancelled = {
      code: "InvalidInput",
      message: "cancelled",
      details: { field: "endpoint", reason: "cancelled" },
    };
    streaming([
      { type: "delta", text: "par" },
      { type: "error", error: cancelled },
    ]);
    await new TauriTransport().ai.generate("req-2", { action: "continue", text: "" }, (event) => seen.push(event));
    expect(seen).toEqual([
      { type: "delta", text: "par" },
      { type: "error", error: cancelled },
    ]);
  });

  it("a command that rejects means nothing was streamed: it rejects with the shell's error and delivers no event", async () => {
    const seen: AiStreamEvent[] = [];
    const busy = { code: "InvalidInput", message: "too many AI requests are running", details: { field: "requestId", reason: "busy" } };
    tauri.invoke.mockRejectedValueOnce(busy);
    const error: unknown = await new TauriTransport().ai
      .generate("req-3", { action: "improve", text: "x" }, (event) => seen.push(event))
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).toApiError()).toEqual(busy);
    expect(seen).toEqual([]);

    tauri.invoke.mockRejectedValueOnce({ code: "InvalidInput", message: "AI is turned off", details: { field: "enabled" } });
    await expect(
      new TauriTransport().ai.chat("chat-3", { messages: [], scope: { type: "all" } }, () => undefined),
    ).rejects.toMatchObject({ details: { field: "enabled" } });
    tauri.invoke.mockRejectedValueOnce("invalid args `input` for command `ai_generate`");
    await expect(
      new TauriTransport().ai.generate("req-4", { action: "improve", text: "x" }, () => undefined),
    ).rejects.toMatchObject({ code: "Internal", message: "invalid args `input` for command `ai_generate`" });
  });

  it("waits for the last event when the command resolves before the channel delivered it", async () => {
    const seen: AiStreamEvent[] = [];
    let channel: { onmessage: (message: unknown) => void } | undefined;
    tauri.invoke.mockImplementationOnce((_command: string, args?: unknown) => {
      channel = (args as { onEvent: { onmessage: (message: unknown) => void } }).onEvent;
      channel.onmessage({ type: "delta", text: "a" });
      return Promise.resolve(undefined);
    });
    let resolved = false;
    const promise = new TauriTransport().ai
      .generate("req-5", { action: "improve", text: "x" }, (event) => seen.push(event))
      .then(() => {
        resolved = true;
      });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(resolved).toBe(false);
    channel?.onmessage({ type: "done", text: "a", model: "m" });
    await promise;
    expect(seen.map((event) => event.type)).toEqual(["delta", "done"]);
  });

  it("reports an error event itself when the last event never comes", async () => {
    vi.useFakeTimers();
    try {
      const seen: AiStreamEvent[] = [];
      streaming([{ type: "delta", text: "a" }]);
      const promise = new TauriTransport().ai.generate("req-6", { action: "improve", text: "x" }, (event) => seen.push(event));
      await vi.advanceTimersByTimeAsync(2500);
      await promise;
      expect(seen).toHaveLength(2);
      expect(seen[1]).toMatchObject({ type: "error", error: { code: "Internal" } });
    } finally {
      vi.useRealTimers();
    }
  });

  it("drops a message that is not an AI stream event and survives a listener that throws", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const seen: AiStreamEvent[] = [];
    streaming([
      "nonsense",
      { type: "delta", text: 3 },
      { type: "sources", sources: [{ n: 1 }] },
      { type: "done", text: "x" },
      { type: "error", error: "no" },
      { type: "delta", text: "ok" },
      { type: "done", text: "ok", model: "m" },
    ]);
    await new TauriTransport().ai.generate("req-7", { action: "improve", text: "x" }, (event) => {
      seen.push(event);
      if (event.type === "delta") {
        throw new Error("listener bug");
      }
    });
    expect(seen.map((event) => event.type)).toEqual(["delta", "done"]);
    // Five messages were refused, one listener failure was logged.
    expect(logged).toHaveBeenCalledTimes(6);
  });

  it("does not share a channel between two streams", async () => {
    streaming([{ type: "done", text: "", model: "m" }]);
    streaming([{ type: "done", text: "", model: "m" }]);
    const ai = new TauriTransport().ai;
    await ai.generate("a", { action: "improve", text: "x" }, () => undefined);
    await ai.chat("b", { messages: [], scope: { type: "all" } }, () => undefined);
    expect(tauri.MockChannel.instances).toHaveLength(2);
  });

  it("cancel invokes ai_cancel with the request id", async () => {
    await expect(new TauriTransport().ai.cancel("req-8")).resolves.toBeUndefined();
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("ai_cancel", { requestId: "req-8" });
  });

  it("search leaves the limit out when there is none and validates the sources", async () => {
    const ai = new TauriTransport().ai;
    tauri.invoke.mockResolvedValueOnce([{ ...sourceRaw, extra: 1 }, { ...sourceRaw, n: 2, blockId: null, icon: null }]);
    const found = await ai.search("launch", { type: "all" }, 6);
    expect(tauri.invoke).toHaveBeenLastCalledWith("ai_search", { query: "launch", scope: { type: "all" }, limit: 6 });
    expect(found).toStrictEqual([sourceRaw, { n: 2, pageId: sourceRaw.pageId, spaceId: SPACE, title: "Launch plan", text: sourceRaw.text, score: 0.5 }]);

    tauri.invoke.mockResolvedValueOnce([]);
    await expect(ai.search("launch", { type: "page", pageId: sourceRaw.pageId })).resolves.toEqual([]);
    expect(tauri.invoke).toHaveBeenLastCalledWith("ai_search", { query: "launch", scope: { type: "page", pageId: sourceRaw.pageId } });
    expect(Object.keys(tauri.invoke.mock.lastCall?.[1] as object)).toEqual(["query", "scope"]);

    for (const wrong of [null, "none", {}, [{ n: 1 }], [{ ...sourceRaw, score: "high" }], [null]]) {
      tauri.invoke.mockResolvedValueOnce(wrong);
      await expect(ai.search("q", { type: "all" }), JSON.stringify(wrong)).rejects.toMatchObject({
        code: "Internal",
        message: "The desktop shell answered ai_search without sources",
      });
    }
  });

  it("transcribe sends the WAV as the raw body with the model in the x-kb-ai header", async () => {
    const ai = new TauriTransport().ai;
    const wav = new Uint8Array([82, 73, 70, 70]);
    tauri.invoke.mockResolvedValueOnce({ text: "Hello there.", language: "English", extra: 1 });
    await expect(ai.transcribe(wav, "r2t2")).resolves.toStrictEqual({ text: "Hello there.", language: "English" });
    expect(tauri.invoke).toHaveBeenLastCalledWith("ai_transcribe", wav, { headers: { "x-kb-ai": '{"model":"r2t2"}' } });
    expect(tauri.invoke.mock.lastCall?.[1]).toBe(wav);

    // No model: the header is an empty object, so the shell uses the chosen one.
    tauri.invoke.mockResolvedValueOnce({ text: "", language: null });
    await expect(ai.transcribe(wav)).resolves.toStrictEqual({ text: "" });
    expect(tauri.invoke).toHaveBeenLastCalledWith("ai_transcribe", wav, { headers: { "x-kb-ai": "{}" } });
  });

  it("keeps the x-kb-ai header ASCII, escaping what is not, and the shell reads the escapes back", async () => {
    tauri.invoke.mockResolvedValueOnce({ text: "" });
    await new TauriTransport().ai.transcribe(new Uint8Array([1]), "café-模型");
    const call = tauri.invoke.mock.lastCall as unknown[] | undefined;
    const headers = (call?.[2] as { headers: Record<string, string> }).headers;
    const header = headers["x-kb-ai"] ?? "";
    expect(header).toMatch(/^[\x20-\x7e]*$/u);
    expect(JSON.parse(header)).toEqual({ model: "café-模型" });
  });

  it("transcribe refuses an answer without text and normalises failures", async () => {
    const ai = new TauriTransport().ai;
    for (const wrong of [null, "text", {}, { text: 1 }, { text: "x", language: 3 }]) {
      tauri.invoke.mockResolvedValueOnce(wrong);
      await expect(ai.transcribe(new Uint8Array([1])), JSON.stringify(wrong)).rejects.toMatchObject({
        code: "Internal",
        message: "The desktop shell answered ai_transcribe without a transcript",
      });
    }
    tauri.invoke.mockRejectedValueOnce({ code: "InvalidInput", message: "choose a transcription model in the AI settings", details: { field: "model", reason: "missing" } });
    await expect(ai.transcribe(new Uint8Array([1]))).rejects.toMatchObject({ details: { field: "model", reason: "missing" } });
    tauri.invoke.mockRejectedValueOnce({ code: "InvalidInput", message: "the audio segment is too large", details: { field: "audio", limit: 4_194_304 } });
    await expect(ai.transcribe(new Uint8Array([1]))).rejects.toMatchObject({ details: { field: "audio" } });
    tauri.invoke.mockRejectedValueOnce({ code: "Locked", message: "the workspace is locked" });
    await expect(ai.status()).rejects.toMatchObject({ code: "Locked" });
  });

  it("works when the methods are called detached from the object", async () => {
    tauri.invoke.mockResolvedValueOnce(undefined);
    // eslint-disable-next-line @typescript-eslint/unbound-method -- detaching is exactly what this test checks
    const { cancel } = new TauriTransport().ai;
    await expect(cancel("x")).resolves.toBeUndefined();
  });
});

describe("TauriTransport.mcp", () => {
  const client = {
    id: "a".repeat(32),
    name: "Claude Desktop",
    grants: [{ spaceId: SPACE, read: true, write: false }],
    createdAt: 1_700_000_000_000,
    lastUsedAt: 1_700_000_100_000,
  };

  it("status resolves with a validated McpStatus", async () => {
    const mcp = new TauriTransport().mcp;
    tauri.invoke.mockResolvedValueOnce({
      enabled: true,
      running: true,
      port: 47_811,
      url: "http://127.0.0.1:47811/mcp",
      bridgePath: "C:/Program Files/atomik-kb/kb-mcp-bridge.exe",
      extra: 1,
    });
    await expect(mcp.status()).resolves.toStrictEqual({
      enabled: true,
      running: true,
      port: 47_811,
      url: "http://127.0.0.1:47811/mcp",
      bridgePath: "C:/Program Files/atomik-kb/kb-mcp-bridge.exe",
    });
    expect(tauri.invoke).toHaveBeenLastCalledWith("mcp_status");

    tauri.invoke.mockResolvedValueOnce({ enabled: true, running: false, port: 47_811, url: "http://127.0.0.1:47811/mcp", error: "the port is in use or cannot be opened", bridgePath: null });
    await expect(mcp.status()).resolves.toStrictEqual({
      enabled: true,
      running: false,
      port: 47_811,
      url: "http://127.0.0.1:47811/mcp",
      error: "the port is in use or cannot be opened",
    });
  });

  it("refuses a status that is not one", async () => {
    const mcp = new TauriTransport().mcp;
    for (const wrong of [undefined, null, "on", {}, { enabled: "yes", running: true, port: 1, url: "u" }, { enabled: true, running: true, port: "1", url: "u" }, { enabled: true, running: true, port: 1, url: "u", error: 4 }]) {
      tauri.invoke.mockResolvedValueOnce(wrong);
      await expect(mcp.status(), JSON.stringify(wrong)).rejects.toMatchObject({
        code: "Internal",
        message: "The desktop shell answered mcp_status without an MCP status",
      });
    }
  });

  it("grantsSet invokes mcp_client_grants_set with the client and the grants and validates the client", async () => {
    const mcp = new TauriTransport().mcp;
    const grants = [{ spaceId: SPACE, read: true, write: true }];
    tauri.invoke.mockResolvedValueOnce({ ...client, grants, extra: true });
    await expect(mcp.grantsSet(client.id, grants)).resolves.toStrictEqual({ ...client, grants });
    expect(tauri.invoke).toHaveBeenCalledExactlyOnceWith("mcp_client_grants_set", { clientId: client.id, grants });

    tauri.invoke.mockResolvedValueOnce({ id: "b", name: "n", grants: [], createdAt: 1, lastUsedAt: null });
    await expect(mcp.grantsSet("b", [])).resolves.toStrictEqual({ id: "b", name: "n", grants: [], createdAt: 1 });

    for (const wrong of [null, "client", { ...client, grants: [{ spaceId: SPACE, read: true }] }, { ...client, createdAt: "now" }, { ...client, grants: "all" }]) {
      tauri.invoke.mockResolvedValueOnce(wrong);
      await expect(mcp.grantsSet(client.id, []), JSON.stringify(wrong)).rejects.toMatchObject({
        code: "Internal",
        message: "The desktop shell answered mcp_client_grants_set without a client",
      });
    }
  });

  it("a declined confirmation rejects with the field grants and the reason declined", async () => {
    const declined = { code: "InvalidInput", message: "the access was not changed", details: { field: "grants", reason: "declined" } };
    tauri.invoke.mockRejectedValueOnce(declined);
    const error: unknown = await new TauriTransport().mcp.grantsSet(client.id, [{ spaceId: SPACE, read: true, write: true }]).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).toApiError()).toEqual(declined);
    tauri.invoke.mockRejectedValueOnce("command mcp_status not found");
    await expect(new TauriTransport().mcp.status()).rejects.toMatchObject({ code: "Internal", message: "command mcp_status not found" });
  });
});
