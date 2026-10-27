import { describe, expect, it } from "vitest";

import { demoPdf } from "./demo-pdf";

const text = (bytes: Uint8Array): string => new TextDecoder().decode(bytes);

describe("demoPdf", () => {
  it("is a small PDF with the pages asked for", () => {
    const pdf = demoPdf(3);
    expect(pdf.length).toBeLessThan(8 * 1024);
    const source = text(pdf);
    expect(source.startsWith("%PDF-1.4\n")).toBe(true);
    expect(source.trimEnd().endsWith("%%EOF")).toBe(true);
    expect(source).toContain("/Count 3");
    expect(source).toContain("page 3 of 3");
    expect(demoPdf(0)).toEqual(demoPdf(1));
  });

  it("has a cross-reference table whose offsets point at its objects", () => {
    const pdf = demoPdf(2);
    const source = text(pdf);
    // Every byte is ASCII, so the text offsets are the byte offsets.
    expect(source.length).toBe(pdf.length);
    const start = Number(/startxref\n(\d+)\n/u.exec(source)?.[1]);
    expect(source.slice(start, start + 4)).toBe("xref");
    const entries = [...source.slice(start).matchAll(/^(\d{10}) 00000 n $/gmu)].map((match) => Number(match[1]));
    expect(entries.length).toBe(4 + 2 * 2);
    entries.forEach((offset, index) => {
      expect(source.slice(offset, offset + `${String(index + 1)} 0 obj`.length)).toBe(`${String(index + 1)} 0 obj`);
    });
  });
});
