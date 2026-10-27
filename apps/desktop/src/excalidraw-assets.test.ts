// The build step that keeps the whiteboard's drawing library off the network (ADR-0033): checked against
// the library that is installed, so a version bump that changes what it expects fails here and not in a
// shipped app.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { excalidrawDist, excalidrawAssets, rewriteFontFallback } from "../excalidraw-assets";

describe("the font fallback of the drawing library", () => {
  const dist = excalidrawDist();
  const chunks = readdirSync(dist).filter((name) => name.endsWith(".js"));

  it("is the one place of the library's production build that names its CDN, and it is rewritten to the app's own path", () => {
    const mentioning = chunks.filter((name) => readFileSync(join(dist, name), "utf8").includes("https://esm.sh/"));
    expect(mentioning).toHaveLength(1);
    const code = readFileSync(join(dist, mentioning[0] ?? ""), "utf8");
    const rewritten = rewriteFontFallback(code);
    expect(rewritten).not.toBeNull();
    expect(rewritten).not.toContain("esm.sh");
    expect(rewritten).toContain('window.location.origin + "/excalidraw/"');
    // The rest of the file is untouched.
    expect(rewritten?.length).toBeLessThan(code.length);
    expect(code.length - (rewritten?.length ?? 0)).toBeLessThan(200);
  });

  it("leaves a file without that address alone and refuses one that has it in another shape", () => {
    expect(rewriteFontFallback("export const a = 1;")).toBeNull();
    expect(() => rewriteFontFallback('fetch("https://esm.sh/x")')).toThrow(/font fallback/u);
  });

  it("is only applied to the library's own files", () => {
    const plugin = excalidrawAssets();
    const transform = plugin.transform as (this: unknown, code: string, id: string) => unknown;
    expect(transform.call({}, 'x = "https://esm.sh/"', "/src/other.js")).toBeNull();
  });
});

describe("the fonts of the drawing library", () => {
  it("are the ones the app serves, a few hundred kilobytes without the CJK family", () => {
    const fonts = readdirSync(join(excalidrawDist(), "fonts"));
    expect(fonts).toEqual(expect.arrayContaining(["Excalifont", "Nunito", "Virgil", "Cascadia", "Liberation", "Lilita", "ComicShanns", "Assistant", "Xiaolai"]));
  });
});
