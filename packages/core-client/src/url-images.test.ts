// The rules of "Image from URL" (ADR-0028) shared by the in-memory transport and the UI: the
// link check, sniffing the type from the bytes, and naming the attachment. The tables are the
// ones of the shell's own tests (`apps/desktop/src-tauri/src/url_import.rs`): both runtimes
// must give the same answers.

import { describe, expect, it } from "vitest";

import { CoreError } from "./errors";
import {
  MAX_IMAGE_URL_CHARS,
  URL_MESSAGES,
  imageExtension,
  imageMime,
  imageName,
  isSvgDocument,
  parseImageUrl,
  sniffImage,
  statusFailure,
  urlFailure,
} from "./url-images";
import type { ImageKind } from "./url-images";
import { mimeForName } from "./shell-names";

const text = (value: string): Uint8Array => new TextEncoder().encode(value);

function bytes(...parts: (number[] | string)[]): Uint8Array {
  return Uint8Array.from(parts.flatMap((part) => (typeof part === "string" ? Array.from(text(part)) : part)));
}

const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
const png = (): Uint8Array => bytes(PNG_SIGNATURE, "\0\0\0\rIHDR rest of the picture");
const jpeg = (): Uint8Array => bytes([0xff, 0xd8, 0xff, 0xe0, 0, 16], "JFIF\0");
const webp = (): Uint8Array => bytes("RIFF\x24\0\0\0WEBPVP8 ");
const bmp = (): Uint8Array => bytes("BM\x46\0\0\0\0\0\0\0\x36\0\0\0", [40, 0, 0, 0], new Array<number>(32).fill(0));
const avif = (brand: string): Uint8Array => bytes("\0\0\0\x1Cftyp", brand, "\0\0\0\0mif1avif");
const SVG = text('<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>');

function details(error: unknown): unknown {
  expect(error).toBeInstanceOf(CoreError);
  return (error as CoreError).details;
}

function refused(link: string): CoreError {
  try {
    parseImageUrl(link);
  } catch (error) {
    expect(error).toBeInstanceOf(CoreError);
    return error as CoreError;
  }
  throw new Error(`${JSON.stringify(link)} was accepted`);
}

describe("parseImageUrl", () => {
  it("accepts an https link and normalises it", () => {
    expect(parseImageUrl("  https://Example.com/a/b.png?x=1#top \n").href).toBe("https://example.com/a/b.png?x=1#top");
    expect(() => parseImageUrl("https://[2001:db8::1]:8443/p.png")).not.toThrow();
  });

  it("refuses everything that is not https, with InvalidInput for the field url and no link in the message", () => {
    for (const link of [
      "http://example.com/a.png",
      "file:///C:/Users/me/a.png",
      "data:image/png;base64,iVBORw0KGgo=",
      "ftp://example.com/a.png",
      "javascript:alert(1)",
      "blob:https://example.com/1234",
      "mailto:me@example.com",
    ]) {
      const error = refused(link);
      expect(error.code, link).toBe("InvalidInput");
      expect(details(error), link).toEqual({ field: "url" });
      expect(error.message, link).not.toContain("example.com");
    }
  });

  it("refuses credentials, empty and malformed links", () => {
    for (const link of [
      "https://user@example.com/a.png",
      "https://user:secret@example.com/a.png",
      "https://:secret@example.com/a.png",
      "",
      "   ",
      "example.com/a.png",
      "//example.com/a.png",
      "https://",
      "https://exa mple.com/a.png",
      "not a link",
    ]) {
      const error = refused(link);
      expect(details(error), JSON.stringify(link)).toEqual({ field: "url" });
      expect(error.message).not.toContain("secret");
    }
  });

  it("accepts at most 2048 characters (characters, not bytes)", () => {
    const prefix = "https://example.com/";
    const atLimit = prefix + "a".repeat(MAX_IMAGE_URL_CHARS - prefix.length);
    expect(() => parseImageUrl(atLimit)).not.toThrow();
    const error = refused(`${atLimit}a`);
    expect(error.message).toContain("2048");
    expect(() => parseImageUrl(prefix + "é".repeat(MAX_IMAGE_URL_CHARS - prefix.length))).not.toThrow();
  });
});

describe("sniffImage", () => {
  const formats: [string, () => Uint8Array, ImageKind][] = [
    ["PNG", png, "png"],
    ["JPEG", jpeg, "jpeg"],
    ["GIF87a", () => bytes("GIF87a\x01\0\x01\0"), "gif"],
    ["GIF89a", () => bytes("GIF89a\x01\0\x01\0"), "gif"],
    ["WebP", webp, "webp"],
    ["BMP", bmp, "bmp"],
    ["AVIF", () => avif("avif"), "avif"],
    ["AVIF sequence", () => avif("avis"), "avif"],
  ];

  it.each(formats)("recognises %s by its signature, whatever the server says", (_name, make, kind) => {
    expect(sniffImage(make())).toBe(kind);
    expect(sniffImage(make(), "text/html")).toBe(kind);
    expect(sniffImage(make(), "image/svg+xml")).toBe(kind);
  });

  it("does not take other things for images", () => {
    const cases: [string, Uint8Array][] = [
      ["empty", new Uint8Array()],
      ["html", text("<!DOCTYPE html><html><body>hi</body></html>")],
      ["json", text('{"error":"not found"}')],
      ["text starting with BM", text("BMW is a car maker, not a bitmap")],
      ["a short PNG head", Uint8Array.from(PNG_SIGNATURE.slice(0, 7))],
      ["zip", bytes("PK\x03\x04\x14\0\0\0")],
      ["pdf", text("%PDF-1.7\n")],
      ["heic", avif("heic")],
      ["wave", bytes("RIFF\x24\0\0\0WAVEfmt ")],
      ["svg without a type", SVG],
    ];
    for (const [what, content] of cases) {
      expect(sniffImage(content), what).toBeUndefined();
      expect(sniffImage(content, "image/png"), what).toBeUndefined();
    }
  });

  it("takes an SVG only when the server says so and the body is an SVG document", () => {
    expect(sniffImage(SVG, "image/svg+xml")).toBe("svg");
    expect(sniffImage(SVG, "Image/SVG+XML; charset=utf-8")).toBe("svg");
    expect(sniffImage(SVG, " image/svg+xml ;x=y")).toBe("svg");
    for (const wrong of ["image/png", "text/xml", "text/html", "application/xml", "", null, undefined]) {
      expect(sniffImage(SVG, wrong), String(wrong)).toBeUndefined();
    }
    expect(sniffImage(text("<html><svg></svg></html>"), "image/svg+xml")).toBeUndefined();
    expect(sniffImage(new Uint8Array(), "image/svg+xml")).toBeUndefined();
    expect(sniffImage(text("<svgfoo/>"), "image/svg+xml")).toBeUndefined();
    expect(sniffImage(text("<SVG/>"), "image/svg+xml")).toBeUndefined();
    expect(sniffImage(text("<svg"), "image/svg+xml")).toBeUndefined();
  });

  it("lets an SVG start with a byte order mark, a declaration, comments and a doctype", () => {
    const ok = (content: Uint8Array | string): boolean => isSvgDocument(typeof content === "string" ? text(content) : content);
    expect(ok(bytes([0xef, 0xbb, 0xbf], "<svg/>"))).toBe(true);
    expect(ok("  \r\n\t<svg>\n</svg>")).toBe(true);
    expect(ok('<?xml version="1.0" encoding="UTF-8"?>\n<svg width="1"/>')).toBe(true);
    expect(ok('<?xml version="1.0"?><!-- made by hand -->\n<svg/>')).toBe(true);
    expect(ok("<!-- one --> <!-- two -->\n<svg/>")).toBe(true);
    expect(
      ok(
        '<?xml version="1.0"?>\n<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd">\n<svg/>',
      ),
    ).toBe(true);
    expect(ok('<!DOCTYPE svg [ <!ENTITY a "x>y"> <!ENTITY b "z"> ]>\n<svg/>')).toBe(true);
    expect(ok('<?xml-stylesheet href="a.css"?><svg/>')).toBe(true);
    expect(ok('<svg\txmlns="http://www.w3.org/2000/svg"/>')).toBe(true);
    expect(ok('<?xml version="1.0"')).toBe(false);
    expect(ok("<!-- never closed <svg/>")).toBe(false);
    expect(ok("<!DOCTYPE svg [ <svg/>")).toBe(false);
    expect(ok(bytes([0xff, 0xfe], "<\0s\0v\0g\0/\0>\0"))).toBe(false);
    expect(ok("hello <svg/>")).toBe(false);
  });
});

describe("imageName", () => {
  const url = (link: string): URL => new URL(link);

  it("is the last path segment with the extension of the sniffed type", () => {
    const cases: [string, ImageKind, string][] = [
      ["https://example.com/photos/sunset.png", "png", "sunset.png"],
      // An extension of another type is replaced.
      ["https://example.com/photos/sunset.jpg", "png", "sunset.png"],
      ["https://example.com/photos/sunset.gif?size=big", "webp", "sunset.webp"],
      // One that names the kind stays, in its own case.
      ["https://example.com/Sunset.JPG", "jpeg", "Sunset.JPG"],
      ["https://example.com/sunset.jpeg", "jpeg", "sunset.jpeg"],
      // None: added.
      ["https://example.com/render", "avif", "render.avif"],
      ["https://example.com/render?id=5#x", "jpeg", "render.jpg"],
      // A script or a version is replaced; digits are not an extension.
      ["https://example.com/image.php", "png", "image.png"],
      ["https://example.com/photo.v2", "png", "photo.png"],
      ["https://example.com/2026.10.09", "png", "2026.10.09.png"],
      ["https://example.com/archive.tar.gz", "png", "archive.tar.png"],
      // Percent-decoded.
      ["https://example.com/my%20holiday%20%28final%29.png", "png", "my holiday (final).png"],
      ["https://example.com/caf%C3%A9.png", "png", "café.png"],
      ["https://example.com/a+b.png", "png", "a+b.png"],
      // Sanitised like a saved file.
      ["https://example.com/a%3Cb%3Ec%3Ad%22e%7Cf%3Fg%2Ah.png", "png", "a_b_c_d_e_f_g_h.png"],
      ["https://example.com/a%2Fb%5Cc.png", "png", "abc.png"],
      ["https://example.com/%00evil%07.png", "png", "evil.png"],
      ["https://example.com/invoice%E2%80%AEfdp.png", "png", "invoicefdp.png"],
      ["https://example.com/CON", "png", "_CON.png"],
      ["https://example.com/nul.txt", "png", "_nul.png"],
      // A broken escape stays as typed; invalid UTF-8 loses what it breaks.
      ["https://example.com/100%.png", "png", "100%.png"],
      ["https://example.com/a%FFb.png", "png", "ab.png"],
    ];
    for (const [link, kind, name] of cases) {
      expect(imageName(url(link), kind), link).toBe(name);
    }
  });

  it("is image.<ext> when the path leaves no name", () => {
    for (const link of [
      "https://example.com",
      "https://example.com/",
      "https://example.com/photos/",
      "https://example.com/.png",
      "https://example.com/.hidden",
      "https://example.com/%20%20",
      "https://example.com/%E2%80%AE.png",
      "https://example.com/..%2F..",
    ]) {
      expect(imageName(url(link), "webp"), link).toBe("image.webp");
    }
    expect(imageName(url("https://example.com/"), "svg")).toBe("image.svg");
    expect(imageName(url("https://example.com/"), "jpeg")).toBe("image.jpg");
  });

  it("cuts a long name to 255 bytes and keeps the extension", () => {
    const name = imageName(url(`https://example.com/${"x".repeat(400)}.jpg`), "png");
    expect(new TextEncoder().encode(name).length).toBeLessThanOrEqual(255);
    expect(name.endsWith(".png")).toBe(true);
    expect(name.startsWith("xxxx")).toBe(true);
  });
});

describe("types and failures", () => {
  it("every kind has a MIME type the shell's own table agrees with", () => {
    for (const kind of ["png", "jpeg", "gif", "webp", "bmp", "avif", "svg"] as const) {
      expect(imageMime(kind)).toMatch(/^image\//u);
      expect(mimeForName(`x.${imageExtension(kind)}`), kind).toBe(imageMime(kind));
    }
  });

  it("failures are InvalidInput for the field url, with the reason and, for a status, its code", () => {
    expect(urlFailure("notImage", URL_MESSAGES.notImage).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the link is not an image",
      details: { field: "url", reason: "notImage" },
    });
    expect(statusFailure(404).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the server answered with an error (HTTP 404)",
      details: { field: "url", reason: "status", status: 404 },
    });
  });
});
