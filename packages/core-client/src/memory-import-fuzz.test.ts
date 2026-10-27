// Whatever a source holds, the import plans, writes and reads back without failing: random Markdown,
// front matter, CSV, HTML and text (a fixed seed, so a failure repeats), through the whole session.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { MemoryTransport } from "./memory";
import { planImport } from "./memory-import-plan";

/** A small deterministic generator (mulberry32). */
function random(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const MARKDOWN_TOKENS = [
  "[", "]", "(", ")", "!", "<", ">", "`", "``", "```", "#", "##", "-", "- ", "* ", "1. ", "> ", "| ", "|", "---", "***", "$$", "$", "~~", "**", "_", "\\", ":", "\"", "'", "[[", "]]", "![", "](", "<aside>", "</aside>", "<details>", "</details>", "<summary>", "<!--", "-->", "<br>", "&amp;",
  "\n", "\n\n", "\n", "  ", "\t", " ", "a", "b", "word", "Title", "img/a.png", "docs/r.pdf", "other.md", "Other.md", "Notes/Other.md", "../x.md", "%20", "%zz", "https://example.com", "https://notion.so/x-0123456789abcdef0123456789abcdef", "mailto:a@b.c", "javascript:alert(1)", "attachment://aaaaaaaa-0000-4000-8000-000000000001", "atomik-kb://page/aaaaaaaa-0000-4000-8000-000000000001", "data:image/png;base64,AAAA", "[ ]", "[x]", "[!NOTE]", "💡", "é", "日本", "![alt](img/a.png)", "![alt](img/a.png \"cap\")", "[doc](docs/r.pdf)", "[l](Other.md)", "[[Other]]", "[[Missing]]", "![remote](https://example.com/a.png)",
];

const YAML_LINES = [
  "title: Fuzz", "title: \"x\"", "icon: 📝", "icon: file.png", "tags: [a, b]", "tags: a b", "key: value", "key: 3", "key: true", "key: 2026-10-07", "key: [x, y]", "key: \"[[Other]]\"", "key:", "  - item", "  - [x] done", "  name: Paris", "  lat: 1.5", "other: \"unterminated", "other: ~", "- stray", "# comment", "Status: Done", "Estimate: soon", "files:", "  - img/a.png",
];

function pick<T>(items: readonly T[], next: () => number): T {
  return items[Math.floor(next() * items.length)] as T;
}

function markdown(next: () => number): string {
  let text = "";
  if (next() < 0.4) {
    text += "---\n";
    for (let n = Math.floor(next() * 6); n > 0; n -= 1) {
      text += `${pick(YAML_LINES, next)}\n`;
    }
    text += next() < 0.8 ? "---\n" : "";
  }
  for (let n = Math.floor(next() * 60); n > 0; n -= 1) {
    text += pick(MARKDOWN_TOKENS, next);
  }
  return text;
}

function csv(next: () => number): string {
  const cells = ["", "a", "b", "1", "2.5", "Yes", "No", "2026-10-07", "October 12, 2026", "x, y", "https://example.com", "a@b.co", "\"quoted, cell\"", "\"un\"closed", "'=1+1", "line\nbreak", "Title (Other.md)", "Name", "Title", "é", ";", ","];
  const delimiter = next() < 0.2 ? ";" : ",";
  const rows = Math.floor(next() * 8);
  const columns = 1 + Math.floor(next() * 5);
  const lines: string[] = [];
  for (let row = 0; row < rows; row += 1) {
    lines.push(Array.from({ length: Math.floor(next() * (columns + 2)) }, () => pick(cells, next)).join(delimiter));
  }
  return lines.join(next() < 0.5 ? "\n" : "\r\n");
}

function html(next: () => number): string {
  const parts = ["<html>", "<head>", "<title>T</title>", "</head>", "<body>", "<h1>H</h1>", "<h1 class=\"page-title\">P", "</h1>", "<div class=\"page-header-icon\">📝</div>", "<p>text &amp; more", "</p>", "<br>", "<script>x", "</script>", "<!-- c -->", "<style>", "</style>", "&#x41;", "&bogus;", "<", ">", "text", "\n"];
  return Array.from({ length: Math.floor(next() * 25) }, () => pick(parts, next)).join("");
}

function sidecar(next: () => number): string {
  const pieces = [
    "{", "}", "\"properties\":[{\"name\":\"X\",\"kind\":\"text\"}]", "\"properties\":[{\"name\":\"Rel\",\"kind\":\"relation\",\"config\":{\"twoWay\":true}}]", "\"properties\":[{\"name\":\"R\",\"kind\":\"rollup\",\"config\":{\"relation\":\"Rel\",\"target\":\"title\",\"function\":\"count_all\"}}]", "\"pages\":[\"a.md\",1]", ",", "[", "]", "null",
  ];
  return Array.from({ length: Math.floor(next() * 6) }, () => pick(pieces, next)).join(next() < 0.5 ? "," : "");
}

describe("an import of anything", () => {
  it("plans without throwing, and always the same plan for the same source", () => {
    for (let seed = 1; seed <= 300; seed += 1) {
      const next = random(seed);
      const files = new Map<string, string>();
      for (let n = Math.floor(next() * 6); n > 0; n -= 1) {
        const dir = pick(["", "", "Notes/", "Notes/Sub/", "Other 0123456789abcdef0123456789abcdef/"], next);
        const kind = next();
        const name = pick(["A", "B", "Other", "Other 0123456789abcdef0123456789abcdef", "Notes", "x y"], next);
        if (kind < 0.55) {
          files.set(`${dir}${name}.md`, markdown(next));
        } else if (kind < 0.7) {
          files.set(`${dir}${name}.csv`, csv(next));
        } else if (kind < 0.8) {
          files.set(`${dir}${name}_all.csv`, csv(next));
        } else if (kind < 0.9) {
          files.set(`${dir}${name}.html`, html(next));
        } else if (kind < 0.95) {
          files.set(`${dir}${name}.txt`, markdown(next));
        } else {
          files.set("atomik-export.json", sidecar(next));
        }
      }
      const binary = ["img/a.png", "docs/r.pdf", "Notes/img/a.png", "Notes/docs/r.pdf"];
      const sourceName = next() < 0.5 ? "Source" : undefined;
      const first = planImport({ files, binaryPaths: binary, sourceName, maxPages: 10_000 });
      const second = planImport({ files, binaryPaths: binary, sourceName, maxPages: 10_000 });
      const shape = (plan: typeof first): string =>
        JSON.stringify([plan.pages, plan.properties, plan.neededFiles, plan.warnings, [...plan.placeholders]]);
      expect(shape(second), `seed ${String(seed)}`).toBe(shape(first));
      // Every page's parent comes before it.
      first.pages.forEach((page, index) => {
        expect(page.parent === null || page.parent < index, `seed ${String(seed)}`).toBe(true);
      });
    }
  });

  it("writes, indexes and reads back every page it planned", async () => {
    const totals = { pages: 0, warnings: 0, attachments: 0, properties: 0 };
    for (let seed = 1; seed <= 120; seed += 1) {
      const next = random(1000 + seed);
      const files: Record<string, string> = {};
      for (let n = 1 + Math.floor(next() * 5); n > 0; n -= 1) {
        const name = pick(["A", "B", "Other", "x y", "Notes/C", "Notes/Sub/D"], next);
        const kind = next();
        if (kind < 0.6) {
          files[`${name}.md`] = markdown(next);
        } else if (kind < 0.8) {
          files[`${name}.csv`] = csv(next);
        } else if (kind < 0.9) {
          files[`${name}.html`] = html(next);
        } else {
          files[`${name}.txt`] = markdown(next);
        }
      }
      let counter = 0;
      const transport = new MemoryTransport({
        newId: () => `00000000-0000-4000-8000-${String(++counter).padStart(12, "0")}`,
        now: () => 1_700_000_000_000,
        workspaceName: "W",
        spaces: [{ name: "Home", pages: [{ title: "Other" }] }],
        importSources: { s: { kind: "folder", files: { ...files, "img/a.png": new Uint8Array([1, 2, 3]), "docs/r.pdf": new Uint8Array([4]) } } },
      });
      const client = new CoreClient(transport);
      const space = (await client.spaceList())[0]?.id ?? "";
      const report = await client.importRun({ spaceId: space, source: "folder" });
      totals.pages += report.pages;
      totals.warnings += (report.warnings?.length ?? 0) + report.moreWarnings;
      totals.attachments += report.attachments;
      totals.properties += report.propertiesCreated;
      const tree = await client.treeGet(space);
      const ids: string[] = [];
      const collect = (nodes: typeof tree.roots): void => {
        for (const node of nodes) {
          ids.push(node.page.id);
          collect(node.children);
        }
      };
      collect(tree.roots);
      expect(ids.length, `seed ${String(seed)}`).toBe(report.pages + 1);
      for (const pageId of ids) {
        await client.pageContentGet(pageId);
        await client.pageExportMarkdown(pageId);
        await client.pagePropertiesGet(pageId);
      }
      await client.search("word");
      // Exporting what was imported works too.
      const exported = await client.exportRun({ spaceId: space, includeChildren: true, format: "markdown" });
      expect(exported.pages, `seed ${String(seed)}`).toBe(ids.length);
    }
    // The generator reaches what it is for: pages, warnings, files and properties.
    expect(totals.pages).toBeGreaterThan(300);
    expect(totals.warnings).toBeGreaterThan(30);
    expect(totals.attachments).toBeGreaterThan(10);
    expect(totals.properties).toBeGreaterThan(30);
  });
});
