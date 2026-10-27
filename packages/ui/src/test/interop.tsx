// Helpers for the tests of import and export (ADR-0036): a transport around a memory core that has the optional
// methods (`exportRun`, `importRun`, `saveTextFile`, the scheduled backup calls) under the test's control, records
// the requests, answers some of them itself and lets a test emit core events (the progress of a run).

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type {
  BackupResult,
  Event as CoreEvent,
  ExportRunResult,
  ImportReport,
  MemoryTransportOptions,
  Request,
  Response,
  Transport,
} from "@atomik-kb/core-client";

import { render } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { onTestFinished } from "vitest";

import { MainLayout } from "../screens/MainLayout";
import { createAppStore } from "../store";
import { StoreContext } from "../store-context";
import { NOW, findMainNavigation } from "./render";

/** The optional methods of a transport that this module lets a test choose. */
export type InteropMethods = Pick<
  Transport,
  "exportRun" | "importRun" | "saveTextFile" | "backupFolderChoose" | "backupRunNow" | "backupExport" | "backupImport" | "drive"
>;

export interface InteropOptions {
  /**
   * Optional methods of the transport; a test gives the ones it wants the shell to have (none: a shell with no
   * files). A function gets the memory core, for methods that act on it (the shell's side of a setting).
   */
  methods?: InteropMethods | ((memory: MemoryTransport) => InteropMethods);
  /**
   * Answers a request itself (`undefined` leaves it to the memory core). It may answer later (a promise, to hold a
   * request back) or throw a `CoreError` (a refusal).
   */
  intercept?: (request: Request) => Response | Promise<Response> | undefined;
}

/** A memory core with the optional import and export methods the test chooses. */
export function interopTransport(memory: MemoryTransport, { methods: chosen = {}, intercept }: InteropOptions = {}) {
  const methods = typeof chosen === "function" ? chosen(memory) : chosen;
  const listeners = new Set<(event: CoreEvent) => void>();
  const requests: Request[] = [];
  const stop = memory.subscribe((event) => {
    for (const listener of [...listeners]) {
      listener(event);
    }
  });
  const transport: Transport = {
    dispatch: (request) => {
      requests.push(request);
      const answer = intercept?.(request);
      return answer === undefined ? memory.dispatch(request) : Promise.resolve(answer);
    },
    subscribe: (onEvent) => {
      listeners.add(onEvent);
      return () => {
        listeners.delete(onEvent);
      };
    },
    coreState: () => memory.coreState(),
    docs: memory.docs,
    attachments: memory.attachments,
    lock: () => memory.lock(),
    unlock: (passphrase) => memory.unlock(passphrase),
    ...methods,
  };
  return {
    transport,
    requests,
    /** Stops listening to the memory core (the store's own stop is the store's). */
    detach: stop,
    /** Delivers a core event to the app, as the shell would. */
    emit: (event: CoreEvent): void => {
      for (const listener of [...listeners]) {
        listener(event);
      }
    },
  };
}

/**
 * Renders the whole layout over a store on a memory core wrapped by `interopTransport`, and waits for the
 * sidebar. The store is the test's, so it can look at it (the print flag, the toasts).
 */
export async function renderInterop(
  memoryOptions: MemoryTransportOptions & { initialRoute?: "home" | "settings" | "diagnostics" } = {},
  options: InteropOptions = {},
) {
  const { initialRoute, ...rest } = memoryOptions;
  const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", ...rest });
  const wrapped = interopTransport(memory, options);
  const store = createAppStore(new CoreClient(wrapped.transport), { corePollMs: 5, initialRoute });
  const stopStore = store.getState().start();
  onTestFinished(() => {
    stopStore();
    wrapped.detach();
  });
  const user = userEvent.setup();
  const view = render(
    <StoreContext value={store}>
      <MainLayout />
    </StoreContext>,
  );
  await findMainNavigation();
  return { memory, store, user, ...wrapped, ...view };
}

/** What a fake `exportRun` answers by default: one page into `Notes.zip`. */
export function exportResult(overrides: Partial<ExportRunResult> = {}): ExportRunResult {
  return { path: "C:\\Users\\Ada\\Documents\\Notes.zip", pages: 3, attachments: 1, warnings: [], ...overrides };
}

/** What a fake `importRun` answers by default. */
export function importReport(overrides: Partial<ImportReport> = {}): ImportReport {
  return {
    rootPageIds: [],
    pages: 4,
    attachments: 2,
    propertiesCreated: 1,
    warnings: [],
    moreWarnings: 0,
    ...overrides,
  };
}

/** A backup result as the shell's "Back up now" answers. */
export function backupResult(path = "C:\\Backups\\atomik-kb-auto-2026-10-12-0930.atomikkb"): BackupResult {
  return {
    summary: { documents: 5, settings: 2, bytes: 4096, createdAt: NOW, accountId: "acc", chunks: 0, chunkBytes: 0 },
    path,
  };
}
