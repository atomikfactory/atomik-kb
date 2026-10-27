// CSV text (ADR-0036, `docs/architecture/import-export.md`, 2.8 and 3.6): the writer of the CSV
// export of a view (RFC 4180, a UTF-8 byte order mark, CRLF line ends, fields quoted when they
// need it, the formula guard) and the reader of CSV imports (quotes, either delimiter, no header
// handling: `./memory-import-plan` takes the header row from the first record).

/** One cell of the CSV export: its text, and whether it came from a number (never guarded). */
export interface CsvCell {
  text: string;
  number?: boolean;
}

/** The first characters of a text cell a spreadsheet would read as a formula. */
const FORMULA_START = /^[=+\-@\t\r]/u;

/** A text cell with the formula guard: a leading `'` when a spreadsheet could take it for a formula. */
export function guardFormula(text: string): string {
  return FORMULA_START.test(text) ? `'${text}` : text;
}

/** One field as RFC 4180 writes it: quoted when it has `,`, `"`, CR or LF, or starts or ends with a space. */
export function csvField(text: string): string {
  const quoted = /[,"\r\n]/u.test(text) || text.startsWith(" ") || text.endsWith(" ");
  return quoted ? `"${text.replaceAll('"', '""')}"` : text;
}

/** The CSV file: a byte order mark, then every record ended by CRLF. Text cells get the formula guard. */
export function formatCsv(rows: readonly (readonly CsvCell[])[]): string {
  const records = rows.map(
    (row) => `${row.map((cell) => csvField(cell.number === true ? cell.text : guardFormula(cell.text))).join(",")}\r\n`,
  );
  return `\u{FEFF}${records.join("")}`;
}

/** The records of a CSV text, and the delimiter that was used. */
export interface ParsedCsv {
  delimiter: "," | ";";
  rows: string[][];
}

/**
 * Reads CSV text: a byte order mark is dropped; the delimiter is `,`, or `;` when the first line
 * has more `;` than `,`; fields in double quotes may hold delimiters, line breaks and doubled
 * quotes; CRLF, LF and CR end records; empty lines are skipped (as the `csv` crate does). A
 * leading `'` before `=`, `+`, `-` or `@` (the export's formula guard) is removed from every field.
 * Records are as long as they are written (the caller pads or cuts them).
 */
export function parseCsv(input: string): ParsedCsv {
  const text = input.replace(/^\u{FEFF}/u, "");
  const firstLine = /^[^\r\n]*/u.exec(text)?.[0] ?? "";
  const semicolons = firstLine.split(";").length - 1;
  const commas = firstLine.split(",").length - 1;
  const delimiter = semicolons > commas ? ";" : ",";
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let quoted = false;
  // Something of the record has been read: an empty line starts nothing and is skipped.
  let started = false;
  const endField = (): void => {
    row.push(unguard(field));
    field = "";
  };
  const endRow = (): void => {
    if (started) {
      endField();
      rows.push(row);
    }
    row = [];
    started = false;
  };
  for (let at = 0; at < text.length; at += 1) {
    const char = text.charAt(at);
    if (quoted) {
      if (char === '"') {
        if (text.charAt(at + 1) === '"') {
          field += '"';
          at += 1;
        } else {
          quoted = false;
        }
      } else {
        field += char;
      }
      continue;
    }
    if (char === '"' && field === "") {
      quoted = true;
      started = true;
    } else if (char === delimiter) {
      started = true;
      endField();
    } else if (char === "\r" || char === "\n") {
      if (char === "\r" && text.charAt(at + 1) === "\n") {
        at += 1;
      }
      endRow();
    } else {
      started = true;
      field += char;
    }
  }
  endRow();
  return { delimiter, rows };
}

/** Removes the export's formula guard: a `'` in front of `=`, `+`, `-` or `@`. */
function unguard(field: string): string {
  return /^'[=+\-@]/u.test(field) ? field.slice(1) : field;
}
