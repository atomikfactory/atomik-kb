import { MemoryTransport } from "@atomik-kb/core-client";
import type { MemoryTransportOptions, Request, Response, Transport } from "@atomik-kb/core-client";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";

import { App } from "../App";
import type { MessageKey } from "../i18n";
import { t } from "../i18n";

/** A fixed clock so timestamps in the UI can be asserted exactly. */
export const NOW = Date.UTC(2026, 9, 3, 12, 0, 0);

export type User = ReturnType<typeof userEvent.setup>;

/** Renders `<App>` over any transport. `strict` mounts inside `<StrictMode>`, which runs effects twice. */
export function renderWith(transport: Transport, { strict = false }: { strict?: boolean } = {}) {
  const user = userEvent.setup();
  const app = <App transport={transport} />;
  const view = render(strict ? <StrictMode>{app}</StrictMode> : app);
  return { user, ...view };
}

/** Renders `<App>` over a fresh in-memory core and returns it together with a user-event driver. */
export function renderApp(options: MemoryTransportOptions & { strict?: boolean } = {}) {
  const { strict, ...memoryOptions } = options;
  const memory = new MemoryTransport({ now: () => NOW, ...memoryOptions });
  return { memory, ...renderWith(memory, { strict }) };
}

/** Waits for the main layout, which only exists once boot finished with a workspace. */
export function findMainNavigation(): Promise<HTMLElement> {
  return screen.findByRole("navigation", { name: t("nav.label") });
}

/** Opens a screen through the sidebar and waits for its heading. */
export async function openScreen(
  user: User,
  item: Extract<MessageKey, "nav.home" | "nav.settings" | "nav.diagnostics">,
): Promise<void> {
  const nav = await findMainNavigation();
  await user.click(within(nav).getByRole("button", { name: t(item) }));
}

/**
 * Wraps a transport to record every request and, on demand, hold responses back so a
 * test can look at the UI while a request is in flight. The app lock, backup and
 * attachment calls (not requests) are passed on, held back too, but not recorded.
 * `attachments: false` leaves the attachment transport out (a shell without one).
 */
export function recordingTransport(inner: MemoryTransport, { attachments = true }: { attachments?: boolean } = {}) {
  const requests: Request[] = [];
  let gate: Promise<void> | undefined;
  const transport: Transport = {
    async dispatch(request: Request): Promise<Response> {
      requests.push(request);
      await gate;
      return inner.dispatch(request);
    },
    subscribe: (onEvent) => inner.subscribe(onEvent),
    coreState: () => inner.coreState(),
    docs: inner.docs,
    async lock() {
      await gate;
      return inner.lock();
    },
    async unlock(passphrase) {
      await gate;
      return inner.unlock(passphrase);
    },
    async backupExport(path) {
      await gate;
      return inner.backupExport(path);
    },
    async backupImport(path, kit) {
      await gate;
      return inner.backupImport(path, kit);
    },
  };
  if (attachments) {
    const files = inner.attachments;
    transport.attachments = {
      async write(attachmentId, bytes) {
        await gate;
        return files.write(attachmentId, bytes);
      },
      async read(attachmentId, range) {
        await gate;
        return files.read(attachmentId, range);
      },
      async importPath(spaceId, path) {
        await gate;
        return files.importPath(spaceId, path);
      },
      async importUrl(spaceId, url) {
        await gate;
        return (
          files.importUrl?.(spaceId, url) ??
          Promise.reject(new Error("the wrapped transport cannot import from a URL"))
        );
      },
      async saveAs(attachmentId, path) {
        await gate;
        return files.saveAs(attachmentId, path);
      },
      async open(attachmentId) {
        await gate;
        return files.open(attachmentId);
      },
    };
  }
  return {
    transport,
    requests,
    /** Holds back every request until the returned function is called. */
    hold(): () => void {
      let release: () => void = () => undefined;
      gate = new Promise<void>((resolve) => {
        release = resolve;
      });
      return () => {
        release();
        gate = undefined;
      };
    },
  };
}
