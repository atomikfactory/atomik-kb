// CSV text (ADR-0036, 2.8 and 3.6): the writer of the CSV export and the reader of CSV imports.

import { describe, expect, it } from "vitest";

import { csvField, formatCsv, guardFormula, parseCsv } from "./memory-csv";

describe("formatCsv", () => {
  it("starts with a byte order mark and ends every record with CRLF", () => {
    const text = formatCsv([[{ text: "Name" }, { text: "Done" }], [{ text: "Milk" }, { text: "Yes" }]]);
    expect(text).toBe("\u{FEFF}Name,Done\r\nMilk,Yes\r\n");
  });

  it("quotes fields with a comma, a quote, CR or LF, or a space at either end, and doubles quotes", () => {
    expect(csvField("plain")).toBe("plain");
    expect(csvField("a,b")).toBe('"a,b"');
    expect(csvField('say "hi"')).toBe('"say ""hi"""');
    expect(csvField("line\nbreak")).toBe('"line\nbreak"');
    expect(csvField("carriage\rreturn")).toBe('"carriage\rreturn"');
    expect(csvField(" lead")).toBe('" lead"');
    expect(csvField("trail ")).toBe('"trail "');
    expect(csvField("in side")).toBe("in side");
    expect(csvField("")).toBe("");
  });

  it("prefixes text cells a spreadsheet could read as a formula with an apostrophe, number cells never", () => {
    expect(guardFormula("=SUM(A1)")).toBe("'=SUM(A1)");
    expect(guardFormula("+1")).toBe("'+1");
    expect(guardFormula("-1")).toBe("'-1");
    expect(guardFormula("@user")).toBe("'@user");
    expect(guardFormula("\tx")).toBe("'\tx");
    expect(guardFormula("\rx")).toBe("'\rx");
    expect(guardFormula("a=b")).toBe("a=b");
    const text = formatCsv([[{ text: "=1+1" }, { text: "-5", number: true }, { text: "-5" }]]);
    expect(text).toBe("\u{FEFF}'=1+1,-5,'-5\r\n");
  });

  it("guards the header too", () => {
    expect(formatCsv([[{ text: "=Evil" }]])).toBe("\u{FEFF}'=Evil\r\n");
  });
});

describe("parseCsv", () => {
  it("reads quoted fields with delimiters, line breaks and doubled quotes", () => {
    const { rows } = parseCsv('Name,Note\r\n"Smith, J","He said ""hi""\nthen left"\r\nplain,x\r\n');
    expect(rows).toEqual([
      ["Name", "Note"],
      ["Smith, J", 'He said "hi"\nthen left'],
      ["plain", "x"],
    ]);
  });

  it("drops a byte order mark and accepts CRLF, LF and CR, with or without a final line end", () => {
    expect(parseCsv("\u{FEFF}a,b\nc,d").rows).toEqual([
      ["a", "b"],
      ["c", "d"],
    ]);
    expect(parseCsv("a,b\rc,d\r").rows).toEqual([
      ["a", "b"],
      ["c", "d"],
    ]);
  });

  it("uses a semicolon when the first line has more of them than commas", () => {
    const parsed = parseCsv("Name;Score\nAnn;3,5\n");
    expect(parsed.delimiter).toBe(";");
    expect(parsed.rows).toEqual([
      ["Name", "Score"],
      ["Ann", "3,5"],
    ]);
    expect(parseCsv("a,b;c\n1,2;3\n").delimiter).toBe(",");
  });

  it("skips empty lines, keeps empty fields and a quoted empty record", () => {
    expect(parseCsv("a,b\n\nc,\n,d\n").rows).toEqual([
      ["a", "b"],
      ["c", ""],
      ["", "d"],
    ]);
    expect(parseCsv('""\n').rows).toEqual([[""]]);
    expect(parseCsv("").rows).toEqual([]);
  });

  it("removes the export's formula guard from every field", () => {
    expect(parseCsv("'=1+1,'-5,'@x,'+3,'plain,'\n").rows).toEqual([["=1+1", "-5", "@x", "+3", "'plain", "'"]]);
  });

  it("round-trips what formatCsv writes, guard and BOM included", () => {
    const rows = [
      [{ text: "Name" }, { text: "Note" }],
      [{ text: "=cmd" }, { text: 'a "b", c\r\nd' }],
      [{ text: " spaced " }, { text: "" }],
    ];
    expect(parseCsv(formatCsv(rows)).rows).toEqual(rows.map((row) => row.map((cell) => cell.text)));
  });
});
