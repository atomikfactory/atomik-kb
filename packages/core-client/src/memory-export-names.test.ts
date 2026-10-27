// File names and the layout of an export (ADR-0036, 2.2): safe names, links between files, the
// layout of an archive (Notion's folders, de-duplication, shared attachments, whiteboards) and the
// single-file rule.

import { describe, expect, it } from "vitest";

import { encodeSegment, layoutExport, relativeHref, safeFileName, safeStem, splitExtension } from "./memory-export-names";
import type { LayoutAttachment, LayoutPage } from "./memory-export-names";

describe("safeStem", () => {
  it("turns characters a file system refuses into spaces and collapses white space", () => {
    expect(safeStem('a<b>c:d"e/f\\g|h?i*j')).toBe("a b c d e f g h i j");
    expect(safeStem("  many   spaces \t here ")).toBe("many spaces here");
    expect(safeStem("line\nbreak\u{7}bell")).toBe("line break bell");
  });

  it("drops trailing dots and spaces, and gives an empty name the name Untitled", () => {
    expect(safeStem("Notes...")).toBe("Notes");
    expect(safeStem("Notes . .")).toBe("Notes");
    expect(safeStem("...")).toBe("Untitled");
    expect(safeStem("")).toBe("Untitled");
    expect(safeStem("<>:")).toBe("Untitled");
  });

  it("appends an underscore to names Windows reserves, with or without an extension", () => {
    expect(safeStem("CON")).toBe("CON_");
    expect(safeStem("nul")).toBe("nul_");
    expect(safeStem("Com1")).toBe("Com1_");
    expect(safeStem("LPT9")).toBe("LPT9_");
    expect(safeStem("con.txt")).toBe("con_.txt");
    expect(safeStem("Console")).toBe("Console");
    expect(safeStem("COM10")).toBe("COM10");
    expect(safeStem("COM0")).toBe("COM0");
  });

  it("cuts the stem to 100 characters on a character boundary and keeps emoji", () => {
    expect(Array.from(safeStem("x".repeat(250)))).toHaveLength(100);
    const emoji = safeStem("😀".repeat(150));
    expect(Array.from(emoji)).toHaveLength(100);
    expect(emoji).toBe("😀".repeat(100));
    expect(safeStem("📝 Weekly review")).toBe("📝 Weekly review");
  });
});

describe("safeFileName and splitExtension", () => {
  it("keeps the extension and makes the stem safe", () => {
    expect(safeFileName("report.pdf")).toBe("report.pdf");
    expect(safeFileName("a/b:c.pdf")).toBe("a b c.pdf");
    expect(safeFileName("notes")).toBe("notes");
    expect(safeFileName("")).toBe("Untitled");
    expect(safeFileName("CON.pdf")).toBe("CON_.pdf");
  });

  it("cuts a long stem before the extension", () => {
    const name = safeFileName(`${"x".repeat(300)}.png`);
    expect(name.endsWith(".png")).toBe(true);
    expect(Array.from(name.slice(0, -4))).toHaveLength(100);
  });

  it("splits at the last dot, never leaving an empty stem", () => {
    expect(splitExtension("archive.tar.gz")).toEqual({ stem: "archive.tar", ext: ".gz" });
    expect(splitExtension(".hidden")).toEqual({ stem: ".hidden", ext: "" });
    expect(splitExtension("plain")).toEqual({ stem: "plain", ext: "" });
  });
});

describe("relative links", () => {
  it("percent-encodes every segment: space, controls and the characters of the spec, not non-ASCII", () => {
    expect(encodeSegment("Weekly review.md")).toBe("Weekly%20review.md");
    expect(encodeSegment("a#b%c'd(e)f<g>h?i[j]k^l`m{n|o}p\"q")).toBe(
      "a%23b%25c%27d%28e%29f%3Cg%3Eh%3Fi%5Bj%5Dk%5El%60m%7Bn%7Co%7Dp%22q",
    );
    expect(encodeSegment("tab\there")).toBe("tab%09here");
    expect(encodeSegment("Café 日本")).toBe("Café%20日本");
  });

  it("is relative to the folder of the file being written, with ../ as needed", () => {
    expect(relativeHref("A.md", "B.md")).toBe("B.md");
    expect(relativeHref("A.md", "A/B.md")).toBe("A/B.md");
    expect(relativeHref("A/B.md", "A/C.md")).toBe("C.md");
    expect(relativeHref("A/B.md", "D.md")).toBe("../D.md");
    expect(relativeHref("A/B/C.md", "A/D/e f.png")).toBe("../D/e%20f.png");
    expect(relativeHref("A/B/C.md", "x.png")).toBe("../../x.png");
    expect(relativeHref("A.md", "A/Weekly review/report.pdf", false)).toBe("A/Weekly review/report.pdf");
  });
});

const page = (id: string, title: string, parentId?: string, extra: Partial<LayoutPage> = {}): LayoutPage => ({
  id,
  parentId,
  title,
  attachments: [],
  whiteboards: 0,
  ...extra,
});

const files = (entries: Record<string, LayoutAttachment>): Map<string, LayoutAttachment> => new Map(Object.entries(entries));

describe("layoutExport", () => {
  it("makes one page without attachments or whiteboards a single file named after it", () => {
    const layout = layoutExport({ pages: [page("a", "Weekly: review?")], attachments: files({}), format: "markdown", exportName: "x" });
    expect(layout.singleFile).toBe(true);
    expect(layout.suggestedName).toBe("Weekly review.md");
    expect(layout.pagePaths.get("a")).toBe("Weekly review.md");
    const html = layoutExport({ pages: [page("a", "Notes")], attachments: files({}), format: "html", exportName: "x" });
    expect(html.suggestedName).toBe("Notes.html");
  });

  it("keeps one page with only unavailable attachments a single file", () => {
    const layout = layoutExport({
      pages: [page("a", "Notes", undefined, { attachments: ["f1", "f2"] })],
      attachments: files({ f1: { name: "gone.pdf", available: false } }),
      format: "markdown",
      exportName: "Notes",
    });
    expect(layout.singleFile).toBe(true);
    expect(layout.attachmentPaths.size).toBe(0);
  });

  it("lays children in the folder of their parent, a top-level page at the top, and names the archive after the export", () => {
    const layout = layoutExport({
      pages: [page("p", "Projects"), page("a", "Alpha", "p"), page("b", "Beta", "p"), page("t", "Todo")],
      attachments: files({}),
      format: "markdown",
      exportName: "Home: space",
    });
    expect(layout.singleFile).toBe(false);
    expect(layout.suggestedName).toBe("Home space.zip");
    expect(Object.fromEntries(layout.pagePaths)).toEqual({
      p: "Projects.md",
      a: "Projects/Alpha.md",
      b: "Projects/Beta.md",
      t: "Todo.md",
    });
  });

  it("de-duplicates per folder, ASCII case-insensitively, pages first: the second Notes.md is Notes (2).md with its folder", () => {
    const layout = layoutExport({
      pages: [
        page("n1", "Notes"),
        page("c1", "Child", "n1"),
        page("n2", "notes"),
        page("c2", "Child", "n2"),
        page("n3", "NOTES"),
        page("c3", "Child", "n3"),
      ],
      attachments: files({}),
      format: "markdown",
      exportName: "x",
    });
    expect(Object.fromEntries(layout.pagePaths)).toEqual({
      n1: "Notes.md",
      c1: "Notes/Child.md",
      n2: "notes (2).md",
      c2: "notes (2)/Child.md",
      n3: "NOTES (3).md",
      c3: "NOTES (3)/Child.md",
    });
  });

  it("writes an attachment shared by several pages once, in the folder of the first page that uses it", () => {
    const layout = layoutExport({
      pages: [
        page("a", "Alpha", undefined, { attachments: ["f1"] }),
        page("b", "Beta", undefined, { attachments: ["f2", "f1"] }),
      ],
      attachments: files({ f1: { name: "report.pdf", available: true }, f2: { name: "report.pdf", available: true } }),
      format: "markdown",
      exportName: "x",
    });
    expect(layout.singleFile).toBe(false);
    expect(layout.attachmentPaths.get("f1")).toBe("Alpha/report.pdf");
    expect(layout.attachmentPaths.get("f2")).toBe("Beta/report.pdf");
    expect(layout.placed.get("a")).toEqual(["f1"]);
    expect(layout.placed.get("b")).toEqual(["f2"]);
    expect(layout.pagePaths.get("a")).toBe("Alpha.md");
  });

  it("de-duplicates attachment names before their extension and puts them after the pages", () => {
    const layout = layoutExport({
      pages: [
        page("a", "Alpha", undefined, { attachments: ["f1", "f2", "f3"] }),
        page("c", "report.pdf", "a"),
        page("d", "report", "a"),
      ],
      attachments: files({
        f1: { name: "report.pdf", available: true },
        f2: { name: "REPORT.pdf", available: true },
        f3: { name: "image.png", available: true },
      }),
      format: "markdown",
      exportName: "x",
    });
    expect(layout.pagePaths.get("c")).toBe("Alpha/report.pdf.md");
    expect(layout.pagePaths.get("d")).toBe("Alpha/report.md");
    expect(layout.attachmentPaths.get("f1")).toBe("Alpha/report.pdf");
    expect(layout.attachmentPaths.get("f2")).toBe("Alpha/REPORT (2).pdf");
    expect(layout.attachmentPaths.get("f3")).toBe("Alpha/image.png");
  });

  it("lays whiteboards out in document order as Whiteboard.excalidraw, Whiteboard (2).excalidraw, ...", () => {
    const layout = layoutExport({
      pages: [page("a", "Alpha", undefined, { whiteboards: 3 })],
      attachments: files({}),
      format: "markdown",
      exportName: "Alpha",
    });
    expect(layout.singleFile).toBe(false);
    expect(layout.whiteboardPaths.get("a")).toEqual([
      "Alpha/Whiteboard.excalidraw",
      "Alpha/Whiteboard (2).excalidraw",
      "Alpha/Whiteboard (3).excalidraw",
    ]);
    expect(layout.suggestedName).toBe("Alpha.zip");
  });

  it("makes names safe in the layout", () => {
    const layout = layoutExport({
      pages: [page("a", "CON"), page("b", ""), page("c", "a/b")],
      attachments: files({}),
      format: "html",
      exportName: "x",
    });
    expect(layout.pagePaths.get("a")).toBe("CON_.html");
    expect(layout.pagePaths.get("b")).toBe("Untitled.html");
    expect(layout.pagePaths.get("c")).toBe("a b.html");
  });
});
