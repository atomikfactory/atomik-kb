// The security side of `MemoryTransport` (Milestone 4): recovery kit, app lock and the
// locked state, device list, security status with keyring counts, the starting options.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import { kitCodeFromEntropy, kitWordsFromEntropy, parseKitCode, parseKitWords } from "./kit";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { BIP39_ENGLISH } from "./wordlist-english";

const START = 1_700_000_000_000;
const PASSPHRASE = "correct horse";

/** Deterministic ids and clock so assertions can name exact values. */
function setup(options: MemoryTransportOptions = {}) {
  let counter = 0;
  let clock = START;
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => clock++,
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  return { transport, client, events };
}

function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

/** The key 00 01 … 1f, for kits a test has to type. */
const KEY = Uint8Array.from({ length: 32 }, (_, index) => index);

const ARGON2 = { memoryKib: 65_536, iterations: 3, parallelism: 4 };
const LOCKED = { code: "Locked", message: "the workspace is locked" };
const LOCK_OFF = { code: "InvalidInput", message: "the app lock is not enabled", details: { field: "lock" } };
const WRONG_PASSPHRASE = { code: "InvalidInput", message: "wrong passphrase", details: { field: "passphrase" } };
const SECURITY_CHANGED: Event = { type: "SecurityChanged" };

describe("SecurityStatus", () => {
  it("reports no account, no kit, the lock off with its Argon2id parameters and nothing in the keyring", async () => {
    const { client } = setup();
    const status = await client.securityStatus();
    expect(status).toEqual({
      appLock: { enabled: false, ...ARGON2 },
      thisDevice: id(1),
      devices: 0,
      keyring: { spaces: 0, documents: 0, attachments: 0, tombstones: 0 },
      rootGeneration: 0,
      rotationPending: false,
      thisDeviceRevoked: false,
      recoveryPublished: false,
    });
    // Unset optional fields are left out, as the core's serialisation does.
    expect(Object.keys(status)).not.toContain("accountId");
    expect(Object.keys(status)).not.toContain("recoveryKit");
  });

  it("counts a space key per space, a document key per structure and page document, a tombstone per purge", async () => {
    const { client, transport } = setup({ workspaceName: "Home" });
    const keyring = async () => (await client.securityStatus()).keyring;
    const space = await client.spaceCreate("Notes");
    expect(await keyring()).toEqual({ spaces: 1, documents: 1, attachments: 0, tombstones: 0 });
    const first = await client.pageCreate({ spaceId: space.id, title: "First" });
    const second = await client.pageCreate({ spaceId: space.id, title: "Second" });
    // Page documents are created on first use, as in the core.
    expect(await keyring()).toMatchObject({ documents: 1 });
    await transport.docs.open(first.id);
    await client.pageContentGet(second.id);
    expect(await keyring()).toEqual({ spaces: 1, documents: 3, attachments: 0, tombstones: 0 });
    await client.pagePurge(second.id);
    expect(await keyring()).toEqual({ spaces: 1, documents: 2, attachments: 0, tombstones: 1 });
    await client.pageTrash(first.id);
    await client.pagePurge(first.id);
    expect(await keyring()).toEqual({ spaces: 1, documents: 1, attachments: 0, tombstones: 2 });
    // Deleting the space deletes its keyring rows, tombstones included.
    await client.spaceDelete(space.id);
    expect(await keyring()).toEqual({ spaces: 0, documents: 0, attachments: 0, tombstones: 0 });
  });

  it("counts a purged subtree page by page and never emits SecurityChanged for keyring hooks", async () => {
    const { client, events } = setup({
      workspaceName: "Home",
      spaces: [{ name: "Notes", pages: [{ title: "Top", children: [{ title: "Child", children: [{ title: "Leaf" }] }] }] }],
    });
    const [top] = (await client.treeGet(id(3))).roots;
    await client.pagePurge(top?.page.id ?? "");
    expect((await client.securityStatus()).keyring).toMatchObject({ spaces: 1, documents: 1, tombstones: 3 });
    expect(events.map((event) => event.type)).not.toContain("SecurityChanged");
  });
});

describe("RecoveryKitCreate", () => {
  it("needs the workspace", async () => {
    const { client, events } = setup();
    expect((await failure(client.recoveryKitCreate())).toApiError()).toEqual({
      code: "InvalidInput",
      message: "create the workspace first",
      details: { field: "workspace" },
    });
    expect(events).toEqual([]);
  });

  it("issues 24 list words and the code of one key, sets the account and the device log", async () => {
    const { client, events } = setup({ workspaceName: "Home" });
    const kit = await client.recoveryKitCreate();
    expect(kit.words).toHaveLength(24);
    expect(kit.words.filter((word) => !BIP39_ENGLISH.includes(word))).toEqual([]);
    expect(kit.code).toMatch(/^(?:[A-Z2-7]{4}-){13}[A-Z2-7]{2}$/u);
    expect(await parseKitWords(kit.words)).toEqual(await parseKitCode(kit.code));
    // Ids: the device (1), the workspace (2), then the account (3).
    expect(kit.accountId).toBe(id(3));
    expect(kit.createdAt).toBe(START + 2);
    expect(events).toEqual([SECURITY_CHANGED]);
    await expect(client.securityStatus()).resolves.toEqual({
      accountId: id(3),
      recoveryKit: { createdAt: START + 2, stale: false },
      appLock: { enabled: false, ...ARGON2 },
      thisDevice: id(1),
      devices: 1,
      keyring: { spaces: 0, documents: 0, attachments: 0, tombstones: 0 },
      rootGeneration: 0,
      rotationPending: false,
      thisDeviceRevoked: false,
      recoveryPublished: false,
    });
  });

  it("issues a different random kit every time", async () => {
    const first = await setup({ workspaceName: "Home" }).client.recoveryKitCreate();
    const second = await setup({ workspaceName: "Home" }).client.recoveryKitCreate();
    expect(second.words).not.toEqual(first.words);
    expect(second.code).not.toBe(first.code);
  });

  it("refuses a second kit with AlreadyExists", async () => {
    const { client, events } = setup({ workspaceName: "Home" });
    const kit = await client.recoveryKitCreate();
    expect((await failure(client.recoveryKitCreate())).toApiError()).toEqual({
      code: "AlreadyExists",
      message: "a recovery kit already exists; re-issue it instead",
    });
    await client.recoveryKitVerify(kit.code);
    expect(events).toEqual([SECURITY_CHANGED, SECURITY_CHANGED]);
  });
});

describe("RecoveryKitVerify", () => {
  it("accepts either form, any case and 4-letter prefixes, and records verifiedAt", async () => {
    const { client, events } = setup({ workspaceName: "Home" });
    const kit = await client.recoveryKitCreate();
    await client.recoveryKitVerify(kit.words.join(" "));
    await client.recoveryKitVerify(`  ${kit.words.map((word) => word.slice(0, 4).toUpperCase()).join("\n")}\n`);
    await client.recoveryKitVerify(kit.code.toLowerCase());
    await client.recoveryKitVerify(kit.code.replaceAll("-", ""));
    const { recoveryKit } = await client.securityStatus();
    expect(recoveryKit?.createdAt).toBe(kit.createdAt);
    expect(recoveryKit?.verifiedAt).toBeGreaterThan(kit.createdAt);
    expect(events).toHaveLength(5);
    expect(events.every((event) => event.type === "SecurityChanged")).toBe(true);
  });

  it("refuses another valid kit and text that does not decode, recording nothing", async () => {
    const { client, events } = setup({ workspaceName: "Home" });
    await client.recoveryKitCreate();
    const other = (await kitWordsFromEntropy(KEY)).join(" ");
    expect((await failure(client.recoveryKitVerify(other))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the kit does not match this workspace",
      details: { field: "kit" },
    });
    expect((await failure(client.recoveryKitVerify("abandon ability"))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "expected 24 words, got 2",
      details: { field: "kit" },
    });
    // Like the core, which only trims: blank text is a code of no characters.
    expect((await failure(client.recoveryKitVerify("   "))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "expected 54 code characters, got 0",
      details: { field: "kit" },
    });
    expect((await client.securityStatus()).recoveryKit).toEqual({ createdAt: START + 2, stale: false });
    expect(events).toEqual([SECURITY_CHANGED]);
  });

  it("never puts the typed kit into the error", async () => {
    const { client } = setup({ workspaceName: "Home" });
    await client.recoveryKitCreate();
    const words = await kitWordsFromEntropy(KEY);
    const typo = [...words.slice(0, 5), "zzzzz", ...words.slice(6)];
    const error = await failure(client.recoveryKitVerify(typo.join(" ")));
    expect(error.message).toBe("word 6 is not in the word list (nor a unique prefix of 4 letters or more)");
    for (const word of typo) {
      expect(JSON.stringify(error.toApiError())).not.toContain(word);
    }
  });

  it("is NotFound before a kit exists, whatever was typed", async () => {
    const { client } = setup({ workspaceName: "Home" });
    const words = (await kitWordsFromEntropy(KEY)).join(" ");
    for (const typed of [words, "not a kit"]) {
      expect((await failure(client.recoveryKitVerify(typed))).toApiError()).toEqual({
        code: "NotFound",
        message: "no recovery kit exists yet",
      });
    }
  });
});

describe("RecoveryKitReissue", () => {
  it("replaces the kit: the old one stops verifying, the account and creation time stay", async () => {
    const { client, events } = setup({ workspaceName: "Home" });
    const first = await client.recoveryKitCreate();
    await client.recoveryKitVerify(first.code);
    const second = await client.recoveryKitReissue();
    expect(second.words).not.toEqual(first.words);
    expect(second.accountId).toBe(first.accountId);
    expect(second.createdAt).toBeGreaterThan(first.createdAt);
    // The new kit is not verified yet.
    expect((await client.securityStatus()).recoveryKit).toEqual({
      createdAt: first.createdAt,
      reissuedAt: second.createdAt,
      stale: false,
    });
    expect((await failure(client.recoveryKitVerify(first.words.join(" ")))).message).toBe(
      "the kit does not match this workspace",
    );
    await client.recoveryKitVerify(second.words.join(" "));
    const status = await client.securityStatus();
    expect(status.recoveryKit?.verifiedAt).toBeGreaterThan(second.createdAt);
    expect(status).toMatchObject({ accountId: first.accountId, devices: 1 });
    expect(events).toEqual([SECURITY_CHANGED, SECURITY_CHANGED, SECURITY_CHANGED, SECURITY_CHANGED]);
  });

  it("wants the app-lock passphrase while the lock is on and ignores it otherwise", async () => {
    const locked = setup({ workspaceName: "Home", security: { kitCreatedAt: START, appLockPassphrase: PASSPHRASE } });
    expect((await failure(locked.client.recoveryKitReissue())).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the app-lock passphrase is required to re-issue the kit",
      details: { field: "passphrase" },
    });
    expect((await failure(locked.client.recoveryKitReissue("wrong horse"))).toApiError()).toEqual(WRONG_PASSPHRASE);
    expect(locked.events).toEqual([]);
    await expect(locked.client.recoveryKitReissue(PASSPHRASE)).resolves.toMatchObject({ accountId: id(3) });

    const open = setup({ workspaceName: "Home", security: { kitCreatedAt: START } });
    await expect(open.client.recoveryKitReissue("anything at all")).resolves.toMatchObject({ accountId: id(3) });
  });

  it("is NotFound before a kit exists", async () => {
    const { client } = setup({ workspaceName: "Home" });
    expect((await failure(client.recoveryKitReissue())).toApiError()).toEqual({
      code: "NotFound",
      message: "no recovery kit exists yet; create one",
    });
  });
});

describe("app lock settings", () => {
  it("enable takes 8 to 256 UTF-8 bytes, reports the lock on and emits SecurityChanged", async () => {
    const { client, events } = setup({ workspaceName: "Home" });
    for (const short of ["", "1234567", "€€"]) {
      expect((await failure(client.appLockEnable(short))).toApiError()).toEqual({
        code: "InvalidInput",
        message: "passphrase must be 8 to 256 bytes",
        details: { field: "passphrase" },
      });
    }
    expect((await failure(client.appLockEnable("x".repeat(257)))).code).toBe("InvalidInput");
    expect(events).toEqual([]);
    await client.appLockEnable("x".repeat(256));
    expect((await client.securityStatus()).appLock).toEqual({ enabled: true, ...ARGON2 });
    expect(events).toEqual([SECURITY_CHANGED]);
  });

  it("enable is AlreadyExists while the lock is on (after validating the passphrase)", async () => {
    const { client } = setup({ security: { appLockPassphrase: PASSPHRASE } });
    expect((await failure(client.appLockEnable("another one"))).toApiError()).toEqual({
      code: "AlreadyExists",
      message: "the app lock is already enabled",
    });
    expect((await failure(client.appLockEnable("short"))).code).toBe("InvalidInput");
  });

  it("disable needs the right passphrase", async () => {
    const { client, events } = setup({ security: { appLockPassphrase: PASSPHRASE } });
    expect((await failure(client.appLockDisable("wrong horse"))).toApiError()).toEqual(WRONG_PASSPHRASE);
    expect((await client.securityStatus()).appLock.enabled).toBe(true);
    await client.appLockDisable(PASSPHRASE);
    expect((await client.securityStatus()).appLock.enabled).toBe(false);
    expect(events).toEqual([SECURITY_CHANGED]);
  });

  it("change reports a bad new passphrase on new and a wrong current one on current", async () => {
    const { client, transport, events } = setup({ security: { appLockPassphrase: PASSPHRASE } });
    expect((await failure(client.appLockChange(PASSPHRASE, "short"))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "must be 8 to 256 bytes",
      details: { field: "new" },
    });
    // The new passphrase is checked first, as in the core.
    expect((await failure(client.appLockChange("wrong horse", "short"))).toApiError()).toMatchObject({
      details: { field: "new" },
    });
    expect((await failure(client.appLockChange("wrong horse", "battery staple"))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "wrong passphrase",
      details: { field: "current" },
    });
    expect(events).toEqual([]);
    await client.appLockChange(PASSPHRASE, "battery staple");
    expect(events).toEqual([SECURITY_CHANGED]);
    await transport.lock();
    expect((await failure(transport.unlock(PASSPHRASE))).toApiError()).toEqual(WRONG_PASSPHRASE);
    await transport.unlock("battery staple");
    await expect(transport.coreState()).resolves.toBe("ready");
  });

  it("disable and change are InvalidInput for lock while the lock is off", async () => {
    const { client } = setup();
    const calls = [
      () => client.appLockDisable(PASSPHRASE),
      () => client.appLockChange(PASSPHRASE, "new one!"),
      // Before the new passphrase is looked at, as in the core.
      () => client.appLockChange(PASSPHRASE, "short"),
    ];
    for (const call of calls) {
      expect((await failure(call())).toApiError()).toEqual(LOCK_OFF);
    }
  });

  it("takes the passphrase as typed: no trimming", async () => {
    const { client } = setup({ security: { appLockPassphrase: ` ${PASSPHRASE} ` } });
    expect((await failure(client.appLockDisable(PASSPHRASE))).toApiError()).toEqual(WRONG_PASSPHRASE);
    await client.appLockDisable(` ${PASSPHRASE} `);
  });
});

describe("locked state", () => {
  const seeded = {
    workspaceName: "Home",
    spaces: [{ name: "Notes", pages: [{ title: "Page" }] }],
    security: { appLockPassphrase: PASSPHRASE },
  } satisfies MemoryTransportOptions;

  it("lock needs the app lock enabled", async () => {
    const { client } = setup({ workspaceName: "Home" });
    expect((await failure(client.lock())).toApiError()).toEqual(LOCK_OFF);
    await expect(client.coreState()).resolves.toBe("ready");
  });

  it("closes the workspace: every request and document call is Locked until unlock", async () => {
    const { client, transport, events } = setup(seeded);
    const pageId = id(4);
    await client.lock();
    await expect(client.coreState()).resolves.toBe("locked");
    const calls: [string, () => Promise<unknown>][] = [
      ["AppStatus", () => client.appStatus()],
      ["WorkspaceGet", () => client.workspaceGet()],
      ["SecurityStatus", () => client.securityStatus()],
      ["DeviceList", () => client.deviceList()],
      ["AppLockDisable", () => client.appLockDisable(PASSPHRASE)],
      ["SettingsSet", () => client.settingsSet("ui.theme", "dark")],
      ["docs.open", () => transport.docs.open(pageId)],
      ["docs.export", () => transport.docs.export(pageId)],
      ["backupExport", () => client.backupExport("/backups/a.atomikkb")],
      ["backupImport", () => client.backupImport("/backups/a.atomikkb")],
    ];
    for (const [name, call] of calls) {
      expect((await failure(call())).toApiError(), name).toEqual(LOCKED);
    }
    expect(events).toEqual([]);
    await client.unlock(PASSPHRASE);
    await expect(client.coreState()).resolves.toBe("ready");
    await expect(client.workspaceGet()).resolves.toMatchObject({ name: "Home" });
    await expect(transport.docs.open(pageId)).resolves.toBeInstanceOf(Uint8Array);
  });

  it("stays locked after a wrong passphrase", async () => {
    const { client } = setup(seeded);
    await client.lock();
    for (const wrong of ["wrong horse", "", PASSPHRASE.toUpperCase(), ` ${PASSPHRASE}`]) {
      expect((await failure(client.unlock(wrong))).toApiError(), wrong).toEqual(WRONG_PASSPHRASE);
    }
    await expect(client.coreState()).resolves.toBe("locked");
    expect((await failure(client.appStatus())).code).toBe("Locked");
  });

  it("locking a locked core succeeds; unlocking a ready one is InvalidInput, like the shell", async () => {
    const { client } = setup(seeded);
    expect((await failure(client.unlock(PASSPHRASE))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the workspace is not locked",
    });
    await client.lock();
    await client.lock();
    await expect(client.coreState()).resolves.toBe("locked");
    await client.unlock(PASSPHRASE);
    await expect(client.coreState()).resolves.toBe("ready");
  });

  it("lock and unlock answer like any request while the core is opening or failed", async () => {
    const failed = { code: "KeystoreUnavailable", message: "no usable keystore" } as const;
    const opening = setup({ ...seeded, coreState: "opening" });
    expect((await failure(opening.client.lock())).message).toBe("core is starting");
    expect((await failure(opening.client.unlock(PASSPHRASE))).message).toBe("core is starting");
    const broken = setup({ ...seeded, coreState: { failed } });
    expect((await failure(broken.client.lock())).toApiError()).toEqual(failed);
    expect((await failure(broken.client.unlock(PASSPHRASE))).toApiError()).toEqual(failed);
  });

  it("can start locked, and setCoreState moves it like the shell would", async () => {
    const { client, transport } = setup({ ...seeded, security: { appLockPassphrase: PASSPHRASE, startLocked: true } });
    await expect(client.coreState()).resolves.toBe("locked");
    expect((await failure(client.appStatus())).toApiError()).toEqual(LOCKED);
    await client.unlock(PASSPHRASE);
    await expect(client.appStatus()).resolves.toMatchObject({ workspaceInitialized: true });
    transport.setCoreState("locked");
    expect((await failure(client.workspaceGet())).code).toBe("Locked");
    const viaCoreState = setup({ ...seeded, coreState: "locked" });
    await expect(viaCoreState.client.coreState()).resolves.toBe("locked");
  });
});

describe("DeviceList", () => {
  it("is empty before the kit, then holds this device under its current name", async () => {
    const { client } = setup({ workspaceName: "Home", deviceName: "Desk" });
    await expect(client.deviceList()).resolves.toEqual([]);
    const kit = await client.recoveryKitCreate();
    const [entry, ...others] = await client.deviceList();
    expect(others).toEqual([]);
    expect(entry).toEqual({
      deviceId: id(1),
      name: "Desk",
      addedAt: kit.createdAt,
      addedViaKit: false,
      isThisDevice: true,
      signingKey: expect.stringMatching(/^[0-9a-f]{64}$/u) as unknown,
    });
    expect(Object.keys(entry ?? {})).not.toContain("revokedAt");
    await client.deviceRename("Laptop");
    await expect(client.deviceList()).resolves.toMatchObject([{ name: "Laptop", isThisDevice: true }]);
    // The signing key is a fixed fake: the same on every call.
    expect((await client.deviceList())[0]?.signingKey).toBe(entry?.signingKey);
  });
});

describe("security options", () => {
  it("can start with a kit: its dates, the account and this device in the log", async () => {
    const { client } = setup({
      workspaceName: "Home",
      security: { kitCreatedAt: START - 5_000, kitVerifiedAt: START - 4_000, kitKey: KEY },
    });
    await expect(client.securityStatus()).resolves.toMatchObject({
      accountId: id(3),
      recoveryKit: { createdAt: START - 5_000, verifiedAt: START - 4_000, stale: false },
      devices: 1,
    });
    await expect(client.deviceList()).resolves.toMatchObject([{ deviceId: id(1), addedAt: START - 5_000 }]);
    // `kitKey` lets a test type the kit, in either form.
    await client.recoveryKitVerify((await kitWordsFromEntropy(KEY)).join(" "));
    await client.recoveryKitVerify(await kitCodeFromEntropy(KEY));
    const other = (await kitWordsFromEntropy(new Uint8Array(32))).join(" ");
    expect((await failure(client.recoveryKitVerify(other))).message).toBe("the kit does not match this workspace");
  });

  it("takes the seeded kit's account id last, so seeded spaces and pages keep their ids", async () => {
    const spaces = [{ name: "Notes", pages: [{ title: "Page" }] }];
    const plain = setup({ workspaceName: "Home", spaces });
    const withKit = setup({ workspaceName: "Home", spaces, security: { kitCreatedAt: START } });
    expect(await withKit.client.spaceList()).toEqual(await plain.client.spaceList());
    expect(await withKit.client.treeGet(id(3))).toEqual(await plain.client.treeGet(id(3)));
    expect((await withKit.client.securityStatus()).accountId).toBe(id(5));
    const named = setup({ workspaceName: "Home", security: { kitCreatedAt: START, accountId: id(99) } });
    expect((await named.client.securityStatus()).accountId).toBe(id(99));
  });

  it("can start with the app lock on", async () => {
    const { client } = setup({ security: { appLockPassphrase: PASSPHRASE } });
    await expect(client.coreState()).resolves.toBe("ready");
    expect((await client.securityStatus()).appLock.enabled).toBe(true);
    await client.lock();
    await client.unlock(PASSPHRASE);
  });

  it("refuses starts the core cannot be in", () => {
    const impossible: [string, MemoryTransportOptions, string][] = [
      ["locked without a passphrase", { security: { startLocked: true } }, "starting locked"],
      ["coreState locked without a passphrase", { coreState: "locked" }, "starting locked"],
      ["a kit without a workspace", { security: { kitCreatedAt: START } }, "needs workspaceName"],
      ["a kit key without a kit", { workspaceName: "Home", security: { kitKey: KEY } }, "need security.kitCreatedAt"],
      [
        "a short kit key",
        { workspaceName: "Home", security: { kitCreatedAt: START, kitKey: new Uint8Array(16) } },
        "must be 32 bytes",
      ],
    ];
    for (const [name, options, message] of impossible) {
      expect(() => new MemoryTransport(options), name).toThrow(message);
    }
    expect(() => new MemoryTransport({ security: { appLockPassphrase: "short" } })).toThrow(
      "passphrase must be 8 to 256 bytes",
    );
  });
});

describe("SecurityChanged", () => {
  it("follows every security change and nothing else", async () => {
    const { client, transport, events } = setup({ workspaceName: "Home" });
    await client.securityStatus();
    await client.deviceList();
    const kit = await client.recoveryKitCreate();
    await client.recoveryKitVerify(kit.code);
    await client.appLockEnable(PASSPHRASE);
    await client.appLockChange(PASSPHRASE, "battery staple");
    await transport.lock();
    await transport.unlock("battery staple");
    await client.recoveryKitReissue("battery staple");
    await client.appLockDisable("battery staple");
    await failure(client.appLockDisable("battery staple"));
    expect(events.map((event) => event.type)).toEqual(Array.from({ length: 6 }, () => "SecurityChanged"));
  });
});
