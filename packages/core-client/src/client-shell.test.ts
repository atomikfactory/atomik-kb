// `CoreClient.onShellEvent`, `hasDesktopShell`, `shellLabelsSet` and the menu bar's `hasMenuBar`, `menuBarShow` and `menuBarHide` (ADR-0040): pass the transport's shell messages and
// labels on, and are quiet no-ops (or, for the labels, an error) for a transport with no shell around it (the
// in-memory one, the web build), so the UI can always listen and knows when it has a shell.

import { describe, expect, it, vi } from "vitest";

import { CoreClient } from "./client";
import { MemoryTransport } from "./memory";
import type { ShellEvent, ShellLabels, Transport } from "./transport";

const PAGE_ID = "12345678-9abc-4def-8123-456789abcdef";

/** The in-memory transport with the two members of a desktop shell, recording what they are given. */
function withShell(memory: MemoryTransport) {
  const listeners = new Set<(event: ShellEvent) => void>();
  const labels: ShellLabels[] = [];
  const transport: Transport = Object.assign(Object.create(memory) as Transport, {
    onShellEvent(listener: (event: ShellEvent) => void): () => void {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    shellLabelsSet(update: ShellLabels): Promise<void> {
      labels.push(update);
      return Promise.resolve();
    },
  });
  return { transport, listeners, labels };
}

describe("CoreClient.onShellEvent", () => {
  it("is a no-op over the in-memory transport: nothing is called and removing it is fine", () => {
    const memory = new MemoryTransport();
    expect("onShellEvent" in memory).toBe(false);
    const listener = vi.fn();
    const stop = new CoreClient(memory).onShellEvent(listener);
    expect(stop).toBeTypeOf("function");
    stop();
    stop();
    expect(listener).not.toHaveBeenCalled();
  });

  it("registers the listener with the transport and returns what removes it", () => {
    const { transport, listeners } = withShell(new MemoryTransport());
    const received: ShellEvent[] = [];
    const stop = new CoreClient(transport).onShellEvent((event) => received.push(event));
    expect(listeners.size).toBe(1);
    for (const listener of listeners) {
      listener({ type: "deepLink", data: { pageId: PAGE_ID } });
      listener({ type: "capture" });
    }
    expect(received).toEqual([{ type: "deepLink", data: { pageId: PAGE_ID } }, { type: "capture" }]);
    stop();
    expect(listeners.size).toBe(0);
  });
});

describe("CoreClient.hasDesktopShell", () => {
  it("is false for a transport with no shell and true for one that has the shell's members", () => {
    const memory = new MemoryTransport();
    expect(new CoreClient(memory).hasDesktopShell()).toBe(false);
    expect(new CoreClient(withShell(memory).transport).hasDesktopShell()).toBe(true);
  });

  it("needs both members", () => {
    const memory = new MemoryTransport();
    const onlyEvents: Transport = Object.assign(Object.create(memory) as Transport, {
      onShellEvent: () => () => undefined,
    });
    const onlyLabels: Transport = Object.assign(Object.create(memory) as Transport, {
      shellLabelsSet: () => Promise.resolve(),
    });
    expect(new CoreClient(onlyEvents).hasDesktopShell()).toBe(false);
    expect(new CoreClient(onlyLabels).hasDesktopShell()).toBe(false);
  });
});

describe("CoreClient menu bar", () => {
  it("has none over a transport without the two members, and a shell with only the labels has none either", async () => {
    const memory = new MemoryTransport();
    expect(new CoreClient(memory).hasMenuBar()).toBe(false);
    expect(new CoreClient(withShell(memory).transport).hasMenuBar()).toBe(false);
    for (const member of ["menuBarShow", "menuBarHide"] as const) {
      const error: unknown = await new CoreClient(memory)[member]().catch((e: unknown) => e);
      expect(error, member).toMatchObject({ code: "Internal" });
    }
  });

  it("needs both members, and hands the calls to the transport", async () => {
    const memory = new MemoryTransport();
    const calls: string[] = [];
    const only: Transport = Object.assign(Object.create(memory) as Transport, {
      menuBarShow: () => Promise.resolve(),
    });
    expect(new CoreClient(only).hasMenuBar()).toBe(false);
    const both: Transport = Object.assign(Object.create(memory) as Transport, {
      menuBarShow: () => {
        calls.push("show");
        return Promise.resolve();
      },
      menuBarHide: () => {
        calls.push("hide");
        return Promise.resolve();
      },
    });
    const client = new CoreClient(both);
    expect(client.hasMenuBar()).toBe(true);
    await client.menuBarShow();
    await client.menuBarHide();
    expect(calls).toEqual(["show", "hide"]);
  });
});

describe("CoreClient.shellLabelsSet", () => {
  it("hands the labels to the transport", async () => {
    const { transport, labels } = withShell(new MemoryTransport());
    await new CoreClient(transport).shellLabelsSet({ open: "Open", quit: "Quit" });
    expect(labels).toEqual([{ open: "Open", quit: "Quit" }]);
  });

  it("rejects when the transport has no shell", async () => {
    const error: unknown = await new CoreClient(new MemoryTransport())
      .shellLabelsSet({ open: "Open" })
      .catch((e: unknown) => e);
    expect(error).toMatchObject({ code: "Internal" });
  });
});
