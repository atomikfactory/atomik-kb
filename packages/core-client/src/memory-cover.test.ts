// Page covers in `MemoryTransport` (ADR-0040): `PageSetCover` with the core's grammar, position, checks
// and messages, the retention of an image cover (it references its attachment like an image icon), undo
// and redo, duplicates, moves between spaces, backups and seeds.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { demoImagePng } from "./demo-image";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import { kitWordsFromEntropy } from "./kit";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";

const DAY = 24 * 60 * 60 * 1000;
const T0 = 1_700_000_000_000;
const UNKNOWN = "00000000-0000-4000-8000-0000000000ff";
const UPPER = "5F0C1D2E-3A4B-4C5D-8E6F-0A1B2C3D4E5F";
const KEY = Uint8Array.from({ length: 32 }, (_, index) => index * 3);

function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

const coverOf = (attachmentId: string): string => `attachment:${attachmentId}`;

/** Deterministic ids, a clock the test moves, a client, the emitted events, the spaces and pages "Alpha" and "Beta". */
async function setup(options: MemoryTransportOptions = {}) {
  let counter = 0;
  const clock = { now: T0 };
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => clock.now,
    workspaceName: "Home",
    spaces: [{ name: "Personal", pages: [{ title: "Alpha" }, { title: "Beta" }] }, { name: "Work" }],
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  const spaces = await client.spaceList();
  const spaceId = (name: string): string => spaces.find((space) => space.name === name)?.id ?? "";
  const personal = spaceId("Personal");
  const roots = personal === "" ? [] : (await client.treeGet(personal)).roots;
  const pageId = (title: string): string => roots.find((node) => node.page.title === title)?.page.id ?? "";
  return { transport, client, events, clock, personal, work: spaceId("Work"), alpha: pageId("Alpha"), beta: pageId("Beta") };
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

/** Refused as `InvalidInput` for `field`; the message. */
async function refusal(promise: Promise<unknown>, field = "cover"): Promise<string> {
  const error = await failure(promise);
  expect(error.toApiError().code).toBe("InvalidInput");
  expect(error.toApiError().details).toEqual({ field });
  return error.message;
}

async function upload(client: CoreClient, spaceId: string, name: string, mime = "image/png"): Promise<string> {
  return (await client.attachmentUpload(spaceId, name, mime, new Blob([name]))).attachmentId;
}

describe("PageSetCover", () => {
  it("sets every form of cover, repositions it and clears it", async () => {
    const { client, alpha, personal } = await setup();
    const photo = await upload(client, personal, "photo.png");
    expect((await client.pageGet(alpha)).cover).toBeUndefined();
    for (const cover of [coverOf(photo), "color:blue", "color:gray", "gradient:sunset"]) {
      const info = await client.pageSetCover(alpha, cover);
      expect(info.cover).toBe(cover);
      expect(info.coverY).toBeUndefined();
      expect(await client.pageGet(alpha)).toEqual(info);
    }

    // The position goes with the cover; 50 is the default and reads as none.
    expect(await client.pageSetCover(alpha, "gradient:sunset", 30)).toMatchObject({ cover: "gradient:sunset", coverY: 30 });
    for (const y of [0, 100]) {
      expect((await client.pageSetCover(alpha, "color:red", y)).coverY).toBe(y);
    }
    expect((await client.pageSetCover(alpha, "color:red", 50)).coverY).toBeUndefined();

    // A position alone changes only the position; a new cover starts again at the default.
    expect(await client.pageSetCover(alpha, null, 20)).toMatchObject({ cover: "color:red", coverY: 20 });
    expect((await client.pageSetCover(alpha, null, 50)).coverY).toBeUndefined();
    await client.pageSetCover(alpha, null, 70);
    const next = await client.pageSetCover(alpha, "color:green");
    expect(next).toMatchObject({ cover: "color:green" });
    expect(next.coverY).toBeUndefined();

    // Trimmed like an icon; the same cover again is not an error.
    expect((await client.pageSetCover(alpha, `  ${coverOf(photo)}  `)).cover).toBe(coverOf(photo));
    await client.pageSetCover(alpha, coverOf(photo));

    // Neither clears both, and clearing again is fine.
    await client.pageSetCover(alpha, "color:red", 10);
    const cleared = await client.pageSetCover(alpha, null);
    expect(cleared.cover).toBeUndefined();
    expect(cleared.coverY).toBeUndefined();
    await client.pageSetCover(alpha, null);
  });

  it("is a separate value from the icon", async () => {
    const { client, alpha } = await setup();
    await client.pageSetIcon(alpha, "🌱");
    expect((await client.pageGet(alpha)).cover).toBeUndefined();
    await client.pageSetCover(alpha, "color:blue");
    await client.pageSetIcon(alpha, null);
    expect(await client.pageGet(alpha)).toMatchObject({ cover: "color:blue" });
    expect((await client.pageGet(alpha)).icon).toBeUndefined();
  });

  it("works on a trashed page and stamps the edit", async () => {
    const { client, clock, alpha } = await setup();
    clock.now = T0 + 5;
    expect((await client.pageSetCover(alpha, "color:blue")).updatedAt).toBe(T0 + 5);
    await client.pageTrash(alpha);
    expect(await client.pageSetCover(alpha, "color:red", 10)).toMatchObject({ cover: "color:red", coverY: 10 });
    await client.pageRestore(alpha);
    expect((await client.pageGet(alpha)).cover).toBe("color:red");
  });

  it("refuses a malformed cover or position with the core's messages and writes nothing", async () => {
    const { client, alpha, beta } = await setup();
    await client.pageSetCover(alpha, "color:blue");
    expect(await refusal(client.pageSetCover(alpha, ""))).toBe("must not be empty; omit it to clear");
    expect(await refusal(client.pageSetCover(alpha, "   "))).toBe("must not be empty; omit it to clear");
    for (const bad of ["🌱", "icon:book", "blue", "https://example.com/a.png", "COLOR:blue"]) {
      expect(await refusal(client.pageSetCover(alpha, bad))).toBe("a cover is attachment:<id>, color:<name> or gradient:<name>");
    }
    for (const bad of ["attachment:", "attachment:abc", `attachment:${UPPER}`, `attachment: ${id(1)}`]) {
      expect(await refusal(client.pageSetCover(alpha, bad))).toBe("the attachment id must be a lowercase hyphenated UUID");
    }
    const colour = await refusal(client.pageSetCover(alpha, "color:teal"));
    expect(colour).toBe("the cover colour must be one of: gray, brown, orange, yellow, green, blue, purple, pink, red");
    expect(await refusal(client.pageSetCover(alpha, "color:"))).toBe(colour);
    expect(await refusal(client.pageSetCover(alpha, "color:Blue"))).toBe(colour);
    expect(await refusal(client.pageSetCover(alpha, "gradient:"))).toBe("a gradient cover is gradient:<name>; the name is missing");
    for (const bad of ["gradient:Sunset", "gradient:sun set", "gradient:sun_set", "gradient:blue:red"]) {
      expect(await refusal(client.pageSetCover(alpha, bad))).toBe("the gradient name may only contain a-z, 0-9 and '-'");
    }
    expect(await refusal(client.pageSetCover(alpha, `gradient:${"x".repeat(33)}`))).toBe(
      "the gradient name must be at most 32 characters",
    );
    await client.pageSetCover(alpha, `gradient:${"x".repeat(32)}`);
    await client.pageSetCover(alpha, "color:blue");

    // A position out of range is refused on its own field, with a cover or alone.
    expect(await refusal(client.pageSetCover(alpha, "color:blue", 101), "coverY")).toBe(
      "the cover position must be between 0 and 100",
    );
    expect(await refusal(client.pageSetCover(alpha, null, 255), "coverY")).toBe("the cover position must be between 0 and 100");
    expect(await refusal(client.pageSetCover(alpha, "color:blue", -1), "coverY")).toBe(
      "the cover position must be between 0 and 100",
    );
    expect(await refusal(client.pageSetCover(alpha, "color:blue", 2.5), "coverY")).toBe(
      "the cover position must be between 0 and 100",
    );
    // A position alone needs a cover to move.
    expect(await refusal(client.pageSetCover(beta, null, 30), "coverY")).toBe("the page has no cover to reposition");

    const unknown = await failure(client.pageSetCover(UNKNOWN, "color:blue"));
    expect(unknown.toApiError().code).toBe("NotFound");
    expect(await client.pageGet(alpha)).toMatchObject({ cover: "color:blue" });
    expect((await client.pageGet(alpha)).coverY).toBeUndefined();
  });

  it("writes nothing and announces nothing when the cover is the one the page has", async () => {
    const { client, events, alpha, personal } = await setup();
    await client.pageSetCover(alpha, "color:blue", 20);
    events.length = 0;
    await client.pageSetCover(alpha, "color:blue", 20);
    await client.pageSetCover(alpha, null, 20);
    expect(events).toEqual([]);
    expect((await client.treeGet(personal)).undo).toEqual({ canUndo: true, canRedo: false });
    await client.pageSetCover(alpha, "color:blue", 21);
    expect(events).toEqual([{ type: "StructureChanged", data: { spaceId: personal } }]);
  });

  it("undoes and redoes the cover and its position as one step", async () => {
    const { client, alpha, personal } = await setup();
    await client.pageSetCover(alpha, "color:blue", 40);
    await client.structureUndo(personal);
    const gone = await client.pageGet(alpha);
    expect(gone.cover).toBeUndefined();
    expect(gone.coverY).toBeUndefined();
    await client.structureRedo(personal);
    expect(await client.pageGet(alpha)).toMatchObject({ cover: "color:blue", coverY: 40 });
    await client.pageSetCover(alpha, null, 80);
    await client.structureUndo(personal);
    expect(await client.pageGet(alpha)).toMatchObject({ cover: "color:blue", coverY: 40 });
  });

  it("must be an image of the page's space and not purged; a refusal changes nothing", async () => {
    const { client, personal, work, alpha } = await setup();
    const elsewhere = await upload(client, work, "w.png");
    const pdf = await upload(client, personal, "r.pdf", "application/pdf");
    const gone = await upload(client, personal, "g.png");
    await client.attachmentDelete(gone);

    expect(await refusal(client.pageSetCover(alpha, coverOf(elsewhere)))).toBe("the image belongs to another space");
    expect(await refusal(client.pageSetCover(alpha, coverOf(pdf)))).toBe(
      "the attachment is not an image (its type must start with image/)",
    );
    const unknown = "the image is not an attachment of this space (unknown or deleted)";
    expect(await refusal(client.pageSetCover(alpha, coverOf(UNKNOWN)))).toBe(unknown);
    expect(await refusal(client.pageSetCover(alpha, coverOf(gone)))).toBe(unknown);
    expect((await client.pageGet(alpha)).cover).toBeUndefined();
    expect((await client.attachmentGet(elsewhere)).references).toBe(0);
    expect((await client.attachmentGet(pdf)).references).toBe(0);

    // Any image type, in any letter case, is fine.
    for (const mime of ["image/jpeg", "image/gif", "image/svg+xml", "IMAGE/PNG"]) {
      const attachmentId = await upload(client, personal, "i", mime);
      expect((await client.pageSetCover(alpha, coverOf(attachmentId))).cover).toBe(coverOf(attachmentId));
    }

    // The cover the page has is not checked again: its image can be moved around after it was deleted.
    const last = (await client.pageGet(alpha)).cover ?? "";
    await client.attachmentDelete(last.slice("attachment:".length));
    expect(await client.pageSetCover(alpha, null, 10)).toMatchObject({ cover: last, coverY: 10 });
  });
});

describe("image covers keep their attachment (ADR-0040)", () => {
  it("referenced through set, replace, clear, trash, purge, undo and redo", async () => {
    const { client, events, clock, personal, alpha, beta } = await setup();
    const a = await upload(client, personal, "a.png");
    const b = await upload(client, personal, "b.png");
    expect(await client.attachmentGet(a)).toMatchObject({ references: 0, unreferencedSince: T0 });
    events.length = 0;

    clock.now = T0 + 10;
    expect((await client.pageSetCover(alpha, coverOf(a))).cover).toBe(coverOf(a));
    expect(events).toContainEqual({ type: "AttachmentsChanged", data: { spaceId: personal } });
    expect(await client.attachmentGet(a)).toMatchObject({ references: 1 });
    expect((await client.attachmentGet(a)).unreferencedSince).toBeUndefined();
    expect(await client.attachmentGet(b)).toMatchObject({ references: 0, unreferencedSince: T0 });

    // One reference per page; the position changes nothing about them.
    await client.pageSetCover(beta, coverOf(a));
    await client.pageSetCover(alpha, null, 25);
    expect((await client.attachmentGet(a)).references).toBe(2);

    // Replacing the cover moves the reference; the other page still holds the first image.
    clock.now = T0 + 20;
    await client.pageSetCover(alpha, coverOf(b));
    expect(await client.attachmentGet(a)).toMatchObject({ references: 1 });
    expect((await client.attachmentGet(a)).unreferencedSince).toBeUndefined();
    expect(await client.attachmentGet(b)).toMatchObject({ references: 1 });

    // Clearing it, or a colour or a gradient, starts the clock.
    clock.now = T0 + 30;
    await client.pageSetCover(beta, null);
    expect(await client.attachmentGet(a)).toMatchObject({ references: 0, unreferencedSince: T0 + 30 });
    clock.now = T0 + 40;
    await client.pageSetCover(alpha, "gradient:sunset");
    expect(await client.attachmentGet(b)).toMatchObject({ references: 0, unreferencedSince: T0 + 40 });

    // A trashed page keeps its cover image; a purge releases it; undo and redo move it back and forth.
    clock.now = T0 + 45;
    await client.pageSetCover(alpha, coverOf(a));
    expect((await client.attachmentGet(a)).unreferencedSince).toBeUndefined();
    await client.pageTrash(alpha);
    expect(await client.attachmentGet(a)).toMatchObject({ references: 1 });
    await client.pageRestore(alpha);
    await client.pageTrash(alpha);
    clock.now = T0 + 50;
    await client.pagePurge(alpha);
    expect(await client.attachmentGet(a)).toMatchObject({ references: 0, unreferencedSince: T0 + 50 });
    await client.structureUndo(personal);
    expect(await client.attachmentGet(a)).toMatchObject({ references: 1 });
    expect((await client.attachmentGet(a)).unreferencedSince).toBeUndefined();
    await client.structureRedo(personal);
    expect(await client.attachmentGet(a)).toMatchObject({ references: 0, unreferencedSince: T0 + 50 });

    // The retention clock is the one of any attachment: both go once 30 days have passed.
    clock.now = T0 + 50 + 30 * DAY + 1;
    expect(await client.attachmentGc()).toMatchObject({ purged: 2 });
  });

  it("swap on undo and redo of a cover change", async () => {
    const { client, clock, personal, alpha } = await setup();
    const a = await upload(client, personal, "a.png");
    await client.pageSetCover(alpha, coverOf(a));
    clock.now = T0 + 5;
    await client.structureUndo(personal);
    expect((await client.pageGet(alpha)).cover).toBeUndefined();
    expect(await client.attachmentGet(a)).toMatchObject({ references: 0, unreferencedSince: T0 + 5 });
    await client.structureRedo(personal);
    expect((await client.pageGet(alpha)).cover).toBe(coverOf(a));
    expect(await client.attachmentGet(a)).toMatchObject({ references: 1 });
    expect((await client.attachmentGet(a)).unreferencedSince).toBeUndefined();
  });

  it("are one reference with the same image as the page's icon, and hold it while either does", async () => {
    const { client, personal, alpha, beta } = await setup();
    const shared = await upload(client, personal, "shared.png");
    await client.pageSetIcon(alpha, coverOf(shared));
    await client.pageSetCover(alpha, coverOf(shared));
    expect((await client.attachmentGet(shared)).references).toBe(1);
    await client.pageSetIcon(alpha, null);
    expect((await client.attachmentGet(shared)).references).toBe(1);
    await client.pageSetCover(alpha, "color:blue");
    expect(await client.attachmentGet(shared)).toMatchObject({ references: 0 });
    expect((await client.attachmentGet(shared)).unreferencedSince).toBeDefined();
    // Two pages are two references.
    await client.pageSetCover(alpha, coverOf(shared));
    await client.pageSetCover(beta, coverOf(shared));
    expect((await client.attachmentGet(shared)).references).toBe(2);
  });

  it("survive an index rebuild, and a restored backup derives them again with the position", async () => {
    const files = new Map<string, Uint8Array>();
    const source = await setup({ files, security: { kitCreatedAt: T0, kitKey: KEY } });
    const a = await upload(source.client, source.personal, "a.png");
    await source.client.pageSetCover(source.alpha, coverOf(a), 35);
    await source.client.indexRebuild();
    expect(await source.client.attachmentGet(a)).toMatchObject({ references: 1 });
    await source.client.backupExport("/backups/covers.atomikkb");

    const target = await setup({ files, workspaceName: undefined, spaces: [] });
    target.clock.now = T0 + 50;
    await target.client.backupImport("/backups/covers.atomikkb", (await kitWordsFromEntropy(KEY)).join(" "));
    expect(await target.client.pageGet(source.alpha)).toMatchObject({ cover: coverOf(a), coverY: 35 });
    expect(await target.client.attachmentGet(a)).toMatchObject({ references: 1 });
    expect((await target.client.attachmentGet(a)).unreferencedSince).toBeUndefined();
  });

  it("count from the start for a seeded page", async () => {
    const attachmentId = id(50);
    const { client, personal } = await setup({
      spaces: [{ name: "Personal", pages: [{ title: "Alpha", cover: coverOf(attachmentId), coverY: 20 }, { title: "Beta", cover: "color:red" }] }],
      attachments: [{ attachmentId, name: "banner.png", mime: "image/png", bytes: demoImagePng() }],
    });
    expect(await client.attachmentGet(attachmentId)).toMatchObject({ references: 1, spaceId: personal });
    expect((await client.attachmentGet(attachmentId)).unreferencedSince).toBeUndefined();
    const roots = (await client.treeGet(personal)).roots;
    expect(roots.map((node) => [node.page.cover, node.page.coverY])).toEqual([[coverOf(attachmentId), 20], ["color:red", undefined]]);
  });
});

describe("covers of duplicated and moved pages", () => {
  it("are copied with a duplicate, with the references and the position", async () => {
    const { client, personal, alpha } = await setup();
    const banner = await upload(client, personal, "banner.png");
    const child = await client.pageCreate({ spaceId: personal, parentId: alpha, title: "Child" });
    await client.pageSetCover(alpha, coverOf(banner), 75);
    await client.pageSetCover(child.id, "gradient:dawn");
    const copy = await client.pageDuplicate(alpha);
    expect(copy).toMatchObject({ cover: coverOf(banner), coverY: 75 });
    expect((await client.attachmentGet(banner)).references).toBe(2);
    const view = await client.treeGet(personal);
    const copied = view.roots.find((node) => node.page.id === copy.id);
    expect(copied?.children.map((node) => [node.page.title, node.page.cover])).toEqual([["Child", "gradient:dawn"]]);
    // The original may go for good: the copy keeps the image.
    await client.pageTrash(alpha);
    await client.pagePurge(alpha);
    expect(await client.attachmentGet(banner)).toMatchObject({ references: 1 });
  });

  it("travel with a move to another space, the image with them when only they use it", async () => {
    const { client, personal, work, alpha, beta } = await setup();
    const banner = await upload(client, personal, "banner.png");
    const common = await upload(client, personal, "common.png");
    const child = await client.pageCreate({ spaceId: personal, parentId: alpha, title: "Child" });
    await client.pageSetCover(alpha, coverOf(banner), 20);
    await client.pageSetCover(child.id, coverOf(common));
    await client.pageSetCover(beta, coverOf(common));

    const summary = await client.pageMoveToSpace(alpha, work, null, null);
    expect(summary).toMatchObject({ attachmentsMoved: 1, attachmentsLeft: 1 });
    expect(await client.pageGet(alpha)).toMatchObject({ spaceId: work, cover: coverOf(banner), coverY: 20 });
    expect(await client.pageGet(child.id)).toMatchObject({ spaceId: work, cover: coverOf(common) });
    expect((await client.attachmentList(work)).map((info) => info.attachmentId)).toEqual([banner]);
    expect((await client.attachmentList(personal)).map((info) => info.attachmentId)).toEqual([common]);
    expect((await client.attachmentGet(banner)).references).toBe(1);
    expect((await client.attachmentGet(common)).references).toBe(2);
    expect((await client.attachmentGet(common)).unreferencedSince).toBeUndefined();
    // Back again: only the image of the source space can move, and only the banner is there.
    const plan = await client.pageMoveToSpacePlan(alpha, personal, null);
    expect(plan).toMatchObject({ attachmentsMoved: 1, attachmentsLeft: 0 });
  });
});
