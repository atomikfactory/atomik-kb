// A small `DriveApi` for the tests of the Google Drive backup target (ADR-0059): a status that the calls change the way the
// shell would, every call a spy, and failures a test chooses. It does not depend on `MemoryDrive` (the dev page's fake).

import { CoreError } from "@atomik-kb/core-client";
import type { DriveApi, DriveClientInput, DriveErrorReason, DriveStatus } from "@atomik-kb/core-client";
import { vi } from "vitest";

/** The folder `FakeDrive` connects to. */
export const DRIVE_FOLDER_URL = "https://drive.google.com/drive/folders/fake-folder";
/** The account `FakeDrive` connects as. */
export const DRIVE_ACCOUNT = "ben@example.com";

/** A status; a built-in client, not connected, by default. */
export function driveStatus(overrides: Partial<DriveStatus> = {}): DriveStatus {
  return { client: "builtIn", connected: false, needsReconnect: false, ...overrides };
}

/** A connected status (the account and the folder as `FakeDrive` gives them). */
export function connectedStatus(overrides: Partial<DriveStatus> = {}): DriveStatus {
  return driveStatus({ connected: true, account: DRIVE_ACCOUNT, folderUrl: DRIVE_FOLDER_URL, ...overrides });
}

/** A failure of a Drive command, as the shell's `ApiError` carries its reason. */
export function driveFailure(reason: DriveErrorReason, message = `drive failed: ${reason}`): CoreError {
  return new CoreError({ code: "InvalidInput", message, details: { reason } });
}

export interface FakeDrive {
  api: DriveApi;
  status: ReturnType<typeof vi.fn<DriveApi["status"]>>;
  connect: ReturnType<typeof vi.fn<DriveApi["connect"]>>;
  disconnect: ReturnType<typeof vi.fn<DriveApi["disconnect"]>>;
  setClient: ReturnType<typeof vi.fn<DriveApi["setClient"]>>;
  cancel: ReturnType<typeof vi.fn<DriveApi["cancel"]>>;
  /** What `status()` answers now. */
  current: () => DriveStatus;
  /** Changes it the way the shell does behind the page's back (the scheduler's delivery, a refused token). */
  change: (next: DriveStatus) => void;
  /** Makes the next `connect()` wait until `finish` or `fail` is called. */
  holdConnect: () => { finish: () => void; fail: (error: unknown) => void };
}

/** A Drive that starts as `initial`: connecting makes it connected, disconnecting and changing the client do as the shell does. */
export function fakeDrive(initial: Partial<DriveStatus> = {}): FakeDrive {
  let state = driveStatus(initial);
  const connected = (): DriveStatus => connectedStatus({ client: state.client, ...(state.ownClientId === undefined ? {} : { ownClientId: state.ownClientId }) });

  const status = vi.fn<DriveApi["status"]>(() => Promise.resolve(state));
  const connect = vi.fn<DriveApi["connect"]>(() => {
    state = connected();
    return Promise.resolve(state);
  });
  const disconnect = vi.fn<DriveApi["disconnect"]>(() => {
    state = driveStatus({ client: state.client, ...(state.ownClientId === undefined ? {} : { ownClientId: state.ownClientId }) });
    return Promise.resolve(state);
  });
  const setClient = vi.fn<(client: DriveClientInput | null) => Promise<DriveStatus>>((client) => {
    state =
      client === null
        ? driveStatus({ client: "builtIn" })
        : driveStatus({ client: "own", ownClientId: client.clientId });
    return Promise.resolve(state);
  });
  const cancel = vi.fn<DriveApi["cancel"]>(() => Promise.resolve());

  const api: DriveApi = {
    status,
    connect,
    disconnect,
    setClient,
    listBackups: () => Promise.resolve([]),
    download: () => Promise.reject(driveFailure("notFound")),
    cancel,
  };

  return {
    api,
    status,
    connect,
    disconnect,
    setClient,
    cancel,
    current: () => state,
    change: (next) => {
      state = next;
    },
    holdConnect: () => {
      let finish: () => void = () => undefined;
      let fail: (error: unknown) => void = () => undefined;
      connect.mockImplementationOnce(
        () =>
          new Promise<DriveStatus>((resolve, reject) => {
            finish = () => {
              state = connected();
              resolve(state);
            };
            fail = reject;
          }),
      );
      return {
        finish: () => {
          finish();
        },
        fail: (error) => {
          fail(error);
        },
      };
    },
  };
}
