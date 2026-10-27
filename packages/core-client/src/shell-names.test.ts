import { describe, expect, it } from "vitest";

import { importName, mimeForName, pathSafeName, runsCode } from "./shell-names";

const ID = "12345678-9abc-4def-8123-456789abcdef";

describe("the shell's file name rules", () => {
  it("importName drops control characters and separators, shortens keeping a short extension, falls back to file", () => {
    expect(importName("Holiday photo.JPG")).toBe("Holiday photo.JPG");
    expect(importName("a\u0007b\\c/d")).toBe("abcd");
    expect(importName("é".repeat(200))).toBe("é".repeat(127));
    expect(importName(`${"x".repeat(300)}.tar.gz`)).toBe(`${"x".repeat(300)}.tar`.slice(0, 252) + ".gz");
    expect(importName(`${"x".repeat(300)}.averyveryverylongextension`)).toBe("x".repeat(255));
    for (const nothing of ["", "\u0000", "..", "."]) {
      expect(importName(nothing)).toBe("file");
    }
  });

  it("mimeForName looks the extension up in the shell's table, any case", () => {
    expect(mimeForName("a.PNG")).toBe("image/png");
    expect(mimeForName("b.tar.gz")).toBe("application/gzip");
    expect(mimeForName("c.docx")).toBe("application/vnd.openxmlformats-officedocument.wordprocessingml.document");
    for (const unknown of ["noext", "d.", "e.unknown", ".hidden"]) {
      expect(mimeForName(unknown), unknown).toBe("application/octet-stream");
    }
  });

  it("pathSafeName makes a stored name safe as a file name on every platform", () => {
    const cases: [string, string][] = [
      ["report.pdf", "report.pdf"],
      ['a<b>c:d"e|f?g*h.txt', "a_b_c_d_e_f_g_h.txt"],
      ["invoice\u202efdp.exe", "invoicefdp.exe"],
      ["zero\u200bwidth", "zerowidth"],
      ["trailing. . ", "trailing"],
      ["CON", "_CON"],
      ["nul.txt", "_nul.txt"],
      ["com1.log", "_com1.log"],
      ["lpt9 .log", "_lpt9 .log"],
      ["CONOUT$", "_CONOUT$"],
      ["console", "console"],
      ["com10", "com10"],
      ["...", ID],
      ["\u0001/\\", ID],
    ];
    for (const [stored, expected] of cases) {
      expect(pathSafeName(stored, ID), JSON.stringify(stored)).toBe(expected);
    }
  });

  it("runsCode spots files the system would run when opened", () => {
    for (const name of ["setup.exe", "RUN.BAT", "x.ps1", "a.lnk", "tool.AppImage", "page.js"]) {
      expect(runsCode(name), name).toBe(true);
    }
    for (const name of ["report.pdf", "photo.png", "exe", "notes.txt"]) {
      expect(runsCode(name), name).toBe(false);
    }
  });
});
