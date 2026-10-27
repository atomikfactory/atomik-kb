// A small valid PDF for tests, built by hand: `pages` pages that each say "Page <n>" in Helvetica,
// optionally with `padding` bytes of an unused stream between the pages (so the file is large
// enough for the viewer to read it in ranges), a classic cross-reference table at the end.

const encoder = new TextEncoder();

export interface TestPdfOptions {
  pages?: number;
  /** Bytes of an unused stream object placed after the first page. */
  padding?: number;
  /** A page size in PDF units. */
  size?: readonly [number, number];
}

/** The bytes of the PDF. */
export function testPdf({ pages = 2, padding = 0, size = [200, 300] }: TestPdfOptions = {}): Uint8Array<ArrayBuffer> {
  const chunks: Uint8Array[] = [];
  const offsets: number[] = [];
  let length = 0;
  const push = (bytes: Uint8Array): void => {
    chunks.push(bytes);
    length += bytes.length;
  };
  const text = (value: string): void => {
    push(encoder.encode(value));
  };
  /** Starts object `number` (1-based). */
  const object = (number: number, body: string): void => {
    offsets[number] = length;
    text(`${String(number)} 0 obj\n${body}\nendobj\n`);
  };

  text("%PDF-1.4\n");
  // 1 catalog, 2 pages, 3 font, then for each page its dictionary and its content.
  object(1, "<< /Type /Catalog /Pages 2 0 R >>");
  const kids = Array.from({ length: pages }, (_, index) => `${String(4 + index * 2)} 0 R`).join(" ");
  object(2, `<< /Type /Pages /Kids [${kids}] /Count ${String(pages)} >>`);
  object(3, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
  let next = 4 + pages * 2;
  for (let index = 0; index < pages; index += 1) {
    const page = 4 + index * 2;
    const content = `BT /F1 24 Tf 20 150 Td (Page ${String(index + 1)}) Tj ET`;
    object(
      page,
      `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 ${String(size[0])} ${String(size[1])}] /Contents ${String(page + 1)} 0 R /Resources << /Font << /F1 3 0 R >> >> >>`,
    );
    object(page + 1, `<< /Length ${String(content.length)} >>\nstream\n${content}\nendstream`);
    if (index === 0 && padding > 0) {
      offsets[next] = length;
      text(`${String(next)} 0 obj\n<< /Length ${String(padding)} >>\nstream\n`);
      push(new Uint8Array(padding).fill(0x20));
      text("\nendstream\nendobj\n");
      next += 1;
    }
  }
  const xref = length;
  const count = next;
  text(`xref\n0 ${String(count)}\n0000000000 65535 f \n`);
  for (let number = 1; number < count; number += 1) {
    text(`${String(offsets[number] ?? 0).padStart(10, "0")} 00000 n \n`);
  }
  text(`trailer\n<< /Size ${String(count)} /Root 1 0 R >>\nstartxref\n${String(xref)}\n%%EOF\n`);

  const out = new Uint8Array(length);
  let at = 0;
  for (const chunk of chunks) {
    out.set(chunk, at);
    at += chunk.length;
  }
  return out;
}
