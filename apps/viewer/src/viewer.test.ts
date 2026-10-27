// The viewer's contract checks (Milestone 13, slice 4): the page's own policy is the server's (`VIEWER_CSP`), the
// page names no remote address and has no inline code, the build is served under the viewer's path, and the errors of
// `kb-wasm` are told apart by their code. The transport against a fake server is `ViewerTransport.test.ts`; the built
// bundle is checked by `dist.test.ts`.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { VIEWER_CSP, VIEWER_PATH } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import { wasmErrorCode } from "./kb-wasm";
import { realWasm } from "./testing";

const root = join(fileURLToPath(new URL(".", import.meta.url)), "..");
const html = readFileSync(join(root, "index.html"), "utf8");

describe("index.html", () => {
  it("carries the policy kb-server sends, word for word", () => {
    const policy = /http-equiv="Content-Security-Policy"\s+content="([^"]*)"/u.exec(html)?.[1];
    expect(policy).toBe(VIEWER_CSP);
  });

  it("names no remote address and runs no inline code", () => {
    // Every address in the page is relative (the page is served under /view and asks its own origin only).
    expect(html).not.toMatch(/(?:https?:)?\/\/[a-z0-9.-]+\.[a-z]{2,}/iu);
    // Scripts are files: an inline script and a handler attribute are what the policy refuses.
    expect(html).not.toMatch(/<script(?![^>]*\bsrc=)[^>]*>/iu);
    expect(html).not.toMatch(/\son[a-z]+=/iu);
    expect(html).not.toMatch(/\sstyle=/iu);
    expect(html).not.toMatch(/<style/iu);
  });

  it("sends no referrer", () => {
    expect(html).toContain('<meta name="referrer" content="no-referrer" />');
  });
});

describe("the build", () => {
  it("is served under the viewer's path, so its addresses are relative to it", () => {
    const config = readFileSync(join(root, "vite.config.ts"), "utf8");
    expect(config).toContain(`base: "${VIEWER_PATH}/"`);
    // Nothing becomes a data: URL, which the policy would refuse.
    expect(config).toContain("assetsInlineLimit: 0");
  });
});

describe("kb-wasm", () => {
  it("tells its errors apart by the code at the front of their message", () => {
    expect(wasmErrorCode(new Error("authentication: the content does not match the link"))).toBe("authentication");
    expect(wasmErrorCode(new Error("bad_key: the link's key is not valid"))).toBe("bad_key");
    expect(wasmErrorCode(new Error("malformed: x"))).toBe("malformed");
    expect(wasmErrorCode(new Error("unknown_attachment: x"))).toBe("unknown_attachment");
    expect(wasmErrorCode(new Error("something else"))).toBeUndefined();
    expect(wasmErrorCode("authentication: not an Error")).toBeUndefined();
  });

  it("raises those errors from the real module, without repeating the key", () => {
    const wasm = realWasm();
    const notAKey = "this is not a key at all, it is far too long";
    let error: unknown;
    try {
      wasm.linkId(notAKey);
    } catch (caught) {
      error = caught;
    }
    expect(wasmErrorCode(error)).toBe("bad_key");
    expect((error as Error).message).not.toContain(notAKey);

    const key = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";
    try {
      wasm.openPublication(key, new Uint8Array([1, 2, 3]));
    } catch (caught) {
      error = caught;
    }
    expect(["malformed", "authentication"]).toContain(wasmErrorCode(error));
    expect((error as Error).message).not.toContain(key);
  });
});
