// A tiny PDF for the in-memory demo (`/dev.html?seed=demo` shows it as a preview in a `file` block),
// built in code so that no binary file is checked in: `pages` letter-size pages of Helvetica text,
// one object per page and a classic cross-reference table, a few kilobytes in all.

const encoder = new TextEncoder();

/** A valid PDF of `pages` pages (at least one), each with a title line and a few lines of text. */
export function demoPdf(pages = 3): Uint8Array<ArrayBuffer> {
  const count = Math.max(1, Math.floor(pages));
  const parts: string[] = [];
  const offsets: number[] = [];
  let length = 0;
  const add = (text: string): void => {
    parts.push(text);
    length += encoder.encode(text).length;
  };
  const object = (number: number, body: string): void => {
    offsets[number] = length;
    add(`${String(number)} 0 obj\n${body}\nendobj\n`);
  };

  add("%PDF-1.4\n");
  // 1 catalog, 2 pages, 3 font, 4 bold font, then a page and its content for each.
  object(1, "<< /Type /Catalog /Pages 2 0 R >>");
  const kids = Array.from({ length: count }, (_, index) => `${String(5 + index * 2)} 0 R`).join(" ");
  object(2, `<< /Type /Pages /Kids [${kids}] /Count ${String(count)} >>`);
  object(3, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
  object(4, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>");
  for (let index = 0; index < count; index += 1) {
    const page = 5 + index * 2;
    const lines = [
      `BT /F2 28 Tf 72 700 Td (atomik-kb demo PDF, page ${String(index + 1)} of ${String(count)}) Tj ET`,
      "BT /F1 14 Tf 72 650 Td 18 TL (This page is drawn by the PDF viewer of the page editor.) Tj T* (Only the pages near the window are drawn.) Tj T* (Select this text to copy it.) Tj ET",
    ].join("\n");
    object(
      page,
      `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents ${String(page + 1)} 0 R /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> >>`,
    );
    object(page + 1, `<< /Length ${String(encoder.encode(lines).length)} >>\nstream\n${lines}\nendstream`);
  }
  const size = 5 + count * 2;
  const xref = length;
  add(`xref\n0 ${String(size)}\n0000000000 65535 f \n`);
  for (let number = 1; number < size; number += 1) {
    add(`${String(offsets[number] ?? 0).padStart(10, "0")} 00000 n \n`);
  }
  add(`trailer\n<< /Size ${String(size)} /Root 1 0 R >>\nstartxref\n${String(xref)}\n%%EOF\n`);
  return encoder.encode(parts.join(""));
}
