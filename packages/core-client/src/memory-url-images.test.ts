// "Image from URL" in `MemoryTransport` (ADR-0028, item 7): `attachments.importUrl` answers from
// the canned `remoteImages` (else `fetch`) with the shell's rules: https only, the type from the
// bytes, the name from the link, failures as `InvalidInput` for the field `url` with a reason.

import { afterEach, describe, expect, it, vi } from "vitest";

import { MAX_PREVIEW_BYTES } from "./attachments";
import { CoreClient } from "./client";
import { demoImagePng } from "./demo-image";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import type { RemoteImage } from "./memory-url-images";

afterEach(() => {
  vi.unstubAllGlobals();
});

const PNG_URL = "https://example.com/photos/demo.png";

function remote(extra: Record<string, RemoteImage> = {}): Map<string, RemoteImage> {
  return new Map<string, RemoteImage>([
    [PNG_URL, { bytes: demoImagePng(), contentType: "image/png" }],
    ["https://example.com/page", { bytes: new TextEncoder().encode("<html>hello</html>".repeat(5)), contentType: "text/html" }],
    [
      "https://example.com/logo.svg",
      { bytes: new TextEncoder().encode('<svg xmlns="http://www.w3.org/2000/svg"/>'), contentType: "image/svg+xml" },
    ],
    [
      "https://example.com/mislabelled.svg",
      { bytes: new TextEncoder().encode('<svg xmlns="http://www.w3.org/2000/svg"/>'), contentType: "text/plain" },
    ],
    ...Object.entries(extra),
  ]);
}

async function setup(options: MemoryTransportOptions = {}) {
  const transport = new MemoryTransport({
    workspaceName: "Home",
    spaces: [{ name: "Personal" }],
    remoteImages: remote(),
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  const [space] = await client.spaceList();
  return { transport, client, events, spaceId: space?.id ?? "" };
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

describe("MemoryTransport attachments.importUrl", () => {
  it("is offered, and the client says so", async () => {
    const { client, transport } = await setup();
    expect(typeof transport.attachments.importUrl).toBe("function");
    expect(client.canImportUrl()).toBe(true);
    expect(client.hasAttachments()).toBe(true);
  });

  it("stores the picture of a canned link as an attachment named after the link, typed from its bytes", async () => {
    const { client, events, spaceId } = await setup();

    const info = await client.attachmentImportUrl(spaceId, PNG_URL);

    expect(info).toMatchObject({ spaceId, name: "demo.png", mime: "image/png", size: demoImagePng().length });
    expect(await client.attachmentRead(info.attachmentId)).toEqual(demoImagePng());
    expect((await client.attachmentList(spaceId)).map((attachment) => attachment.attachmentId)).toEqual([info.attachmentId]);
    expect(events.filter((event) => event.type === "AttachmentsChanged")).toEqual([
      { type: "AttachmentsChanged", data: { spaceId } },
    ]);
  });

  it("finds a canned link as typed (trimmed) or as parsed", async () => {
    const typed = "https://example.com";
    const { client, spaceId } = await setup({
      remoteImages: remote({ [typed]: { bytes: demoImagePng(), contentType: "image/png" } }),
    });
    expect((await client.attachmentImportUrl(spaceId, `  ${PNG_URL}\n`)).name).toBe("demo.png");
    // `https://example.com` parses to `https://example.com/`: the typed form is what the map holds.
    expect((await client.attachmentImportUrl(spaceId, typed)).name).toBe("image.png");
    expect((await client.attachmentImportUrl(spaceId, "https://EXAMPLE.com/photos/demo.png")).name).toBe("demo.png");
  });

  it("names the attachment with the extension of the type the bytes show, not the one the link or server says", async () => {
    const png = { bytes: demoImagePng(), contentType: "application/octet-stream" };
    const { client, spaceId } = await setup({
      remoteImages: remote({
        "https://example.com/render.php?id=7": png,
        "https://example.com/sunset.jpg": png,
        "https://example.com/my%20photo%20%281%29.JPG": png,
        "https://example.com/": png,
      }),
    });
    for (const [link, name] of [
      ["https://example.com/render.php?id=7", "render.png"],
      ["https://example.com/sunset.jpg", "sunset.png"],
      ["https://example.com/my%20photo%20%281%29.JPG", "my photo (1).png"],
      ["https://example.com/", "image.png"],
    ] as const) {
      const info = await client.attachmentImportUrl(spaceId, link);
      expect(info.name, link).toBe(name);
      expect(info.mime, link).toBe("image/png");
    }
  });

  it("takes an SVG only when the server said image/svg+xml", async () => {
    const { client, spaceId } = await setup();
    await expect(client.attachmentImportUrl(spaceId, "https://example.com/logo.svg")).resolves.toMatchObject({
      name: "logo.svg",
      mime: "image/svg+xml",
    });
    const error = await failure(client.attachmentImportUrl(spaceId, "https://example.com/mislabelled.svg"));
    expect(error.toApiError()).toEqual({
      code: "InvalidInput",
      message: "the link is not an image",
      details: { field: "url", reason: "notImage" },
    });
  });

  it("refuses a page: InvalidInput for url, reason notImage, and stores nothing", async () => {
    const { client, spaceId, events } = await setup();
    const error = await failure(client.attachmentImportUrl(spaceId, "https://example.com/page"));
    expect(error.toApiError()).toEqual({
      code: "InvalidInput",
      message: "the link is not an image",
      details: { field: "url", reason: "notImage" },
    });
    expect(await client.attachmentList(spaceId)).toEqual([]);
    expect(events).toEqual([]);
  });

  it("refuses links that are not https, with credentials, empty or too long, naming the field url and never the link", async () => {
    const { client, spaceId } = await setup();
    for (const link of [
      "http://example.com/photos/demo.png",
      "file:///C:/demo.png",
      "data:image/png;base64,AAAA",
      "https://user:secret@example.com/photos/demo.png",
      "",
      "not a link",
      `https://example.com/${"a".repeat(2100)}`,
    ]) {
      const error = await failure(client.attachmentImportUrl(spaceId, link));
      expect(error.code, link).toBe("InvalidInput");
      expect(error.details, link).toEqual({ field: "url" });
      expect(error.message, link).not.toContain("example.com");
      expect(error.message, link).not.toContain("secret");
    }
    expect(await client.attachmentList(spaceId)).toEqual([]);
  });

  it("refuses a canned picture above the preview limit, with details.limit", async () => {
    const { client, spaceId } = await setup({
      remoteImages: remote({
        "https://example.com/huge.png": { bytes: new Uint8Array(MAX_PREVIEW_BYTES + 1), contentType: "image/png" },
      }),
    });
    const error = await failure(client.attachmentImportUrl(spaceId, "https://example.com/huge.png"));
    expect(error.toApiError()).toMatchObject({
      code: "InvalidInput",
      details: { field: "size", limit: MAX_PREVIEW_BYTES },
    });
  });

  it("answers like the shell while the core is locked or starting, and serves injected failures", async () => {
    const locked = new MemoryTransport({
      workspaceName: "Home",
      remoteImages: remote(),
      security: { appLockPassphrase: "correct horse battery", startLocked: true },
    });
    const refused = await failure(
      locked.attachments.importUrl?.("00000000-0000-4000-8000-0000000000ff", PNG_URL) ?? Promise.resolve(),
    );
    expect(refused.code).toBe("Locked");

    const { client, transport, spaceId } = await setup();
    transport.failNextAttachments({ code: "Storage", message: "the chunk could not be stored" });
    expect((await failure(client.attachmentImportUrl(spaceId, PNG_URL))).code).toBe("Storage");
    await expect(client.attachmentImportUrl(spaceId, PNG_URL)).resolves.toMatchObject({ name: "demo.png" });
  });

  it("an unknown space is NotFound and leaves no upload behind", async () => {
    const { client, spaceId } = await setup();
    const error = await failure(client.attachmentImportUrl("00000000-0000-4000-8000-0000000000ff", PNG_URL));
    expect(error.code).toBe("NotFound");
    expect(await client.attachmentList(spaceId)).toEqual([]);
  });
});

describe("MemoryTransport attachments.importUrl without a canned answer", () => {
  function response(bytes: Uint8Array<ArrayBuffer>, init: ResponseInit & { url?: string; redirected?: boolean } = {}): Response {
    const { url, redirected, ...rest } = init;
    const made = new Response(bytes, rest);
    Object.defineProperty(made, "url", { value: url ?? PNG_URL });
    Object.defineProperty(made, "redirected", { value: redirected ?? false });
    return made;
  }

  it("fetches the link with no credentials and no referrer and stores what comes back", async () => {
    const fetchSpy = vi.fn(() =>
      Promise.resolve(response(demoImagePng(), { headers: { "content-type": "image/png" } })),
    );
    vi.stubGlobal("fetch", fetchSpy);
    const { client, spaceId } = await setup({ remoteImages: new Map() });

    const info = await client.attachmentImportUrl(spaceId, PNG_URL);

    expect(info).toMatchObject({ name: "demo.png", mime: "image/png" });
    expect(fetchSpy).toHaveBeenCalledOnce();
    const [address, init] = fetchSpy.mock.calls[0] as unknown as [string, RequestInit];
    expect(address).toBe(PNG_URL);
    expect(init).toMatchObject({
      credentials: "omit",
      referrerPolicy: "no-referrer",
      redirect: "follow",
      headers: { Accept: "image/*" },
    });
  });

  it("reports a status, a network failure, a redirect to http and a too-large body as the shell does", async () => {
    const { client, spaceId } = await setup({ remoteImages: new Map() });

    vi.stubGlobal("fetch", () => Promise.resolve(response(new Uint8Array(), { status: 404 })));
    expect((await failure(client.attachmentImportUrl(spaceId, PNG_URL))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the server answered with an error (HTTP 404)",
      details: { field: "url", reason: "status", status: 404 },
    });

    vi.stubGlobal("fetch", () => Promise.reject(new TypeError("Failed to fetch")));
    expect((await failure(client.attachmentImportUrl(spaceId, PNG_URL))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the server could not be reached",
      details: { field: "url", reason: "network" },
    });

    vi.stubGlobal("fetch", () =>
      Promise.resolve(response(demoImagePng(), { redirected: true, url: "http://cdn.example.com/demo.png" })),
    );
    expect((await failure(client.attachmentImportUrl(spaceId, PNG_URL))).toApiError()).toMatchObject({
      details: { field: "url", reason: "redirect" },
    });

    vi.stubGlobal("fetch", () =>
      Promise.resolve(response(demoImagePng(), { headers: { "content-length": String(MAX_PREVIEW_BYTES + 1) } })),
    );
    expect((await failure(client.attachmentImportUrl(spaceId, PNG_URL))).toApiError()).toMatchObject({
      details: { field: "size", limit: MAX_PREVIEW_BYTES },
    });

    vi.stubGlobal("fetch", () =>
      Promise.resolve(response(new TextEncoder().encode("<html>".repeat(20)), { headers: { "content-type": "text/html" } })),
    );
    expect((await failure(client.attachmentImportUrl(spaceId, PNG_URL))).toApiError()).toMatchObject({
      details: { field: "url", reason: "notImage" },
    });
  });

  it("makes no request for a link the shell would refuse", async () => {
    const fetchSpy = vi.fn(() => Promise.resolve(response(demoImagePng())));
    vi.stubGlobal("fetch", fetchSpy);
    const { client, spaceId } = await setup({ remoteImages: new Map() });
    await failure(client.attachmentImportUrl(spaceId, "http://example.com/demo.png"));
    expect(fetchSpy).not.toHaveBeenCalled();
  });
});
