// The template button (`templateButton`, ADR-0056) and the `relative` attribute of the mentions in the TypeScript twins of
// `kb-model`: the readers of a page document (blocks, links, date and person mentions), the Markdown and the HTML of a page, the
// text the AI index chunks, and the in-memory core's search, backlinks, reminders and first images. The content of a button is
// template material: none of it is the page's text, link, reminder or picture, and all of it is still in the document.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { renderMarkdown } from "./markdown";
import { MemoryTransport } from "./memory";
import { blocksOfPortable, chunkPage } from "./memory-ai-text";
import { htmlPage } from "./memory-export-text";
import {
  extractAttachmentRefs,
  extractBlocks,
  extractDateMentions,
  extractLinks,
  extractPersonMentions,
  loroToPortable,
  newPageDoc,
  portableToLoro,
} from "./page-doc";

function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

const text = (words: string): JsonValue => ({ type: "text", text: words });
const ref = (n: number, target: number): JsonValue => ({ type: "pageRef", attrs: { id: id(n), pageId: id(target) } });
const date = (n: number, day: string, extra: Record<string, JsonValue> = {}): JsonValue => ({
  type: "dateMention",
  attrs: { id: id(n), date: day, ...extra },
});
const person = (n: number, label: string, extra: Record<string, JsonValue> = {}): JsonValue => ({
  type: "personMention",
  attrs: { accountId: id(n), label, ...extra },
});
const para = (n: number, content: JsonValue[]): JsonValue => ({ type: "paragraph", attrs: { id: id(n) }, content });
const image = (n: number, attachment: number, alt = "hidden alt", caption = "hidden caption"): JsonValue => ({
  type: "image",
  attrs: { id: id(n), attachmentId: id(attachment), alt, caption },
});
const button = (n: number, attrs: Record<string, JsonValue>, content: JsonValue[]): JsonValue => ({
  type: "templateButton",
  attrs: { id: id(n), ...attrs },
  content,
});

/**
 * `Visible`, a link, a date with a reminder and a person; a button holding the same kinds of things (and a button of its own);
 * `After`. The links go to the pages 4 (outside) and 5 (inside).
 */
function page(options: { imageOutside?: boolean } = {}): JsonValue {
  const inside: JsonValue[] = [
    para(11, [
      text("Hidden words "),
      ref(12, 5),
      text(" due "),
      date(13, "2026-10-07", { time: "14:00", reminder: 15, relative: "today" }),
      person(1, "Ada", { relative: "me" }),
    ]),
    image(14, 200),
    button(15, {}, [para(16, [text("Nested")])]),
  ];
  const content: JsonValue[] = [
    para(1, [text("Visible words "), ref(2, 4), text(" "), date(3, "2026-10-07", { time: "14:30", reminder: 15 }), person(2, "Grace")]),
    button(10, { label: "Add task", position: "above" }, inside),
    para(20, [text("After")]),
  ];
  if (options.imageOutside === true) {
    content.push(image(21, 201, "picture alt", "picture caption"));
  }
  return { type: "doc", content };
}

function loaded(content: JsonValue) {
  const doc = newPageDoc();
  portableToLoro(doc, content);
  return doc;
}

const noTitle = (): undefined => undefined;

describe("the readers of a page document", () => {
  it("keep the attributes of a button and of a relative mention through the document", () => {
    const doc = loaded(page());
    expect(loroToPortable(doc)).toEqual(page());
  });

  it("list the blocks of a button with no text, marked as template material, and keep their ids and attachments", () => {
    const blocks = extractBlocks(loaded(page()));
    expect(blocks.map((block) => block.kind)).toEqual([
      "paragraph",
      "pageRef",
      "dateMention",
      "templateButton",
      "paragraph",
      "pageRef",
      "dateMention",
      "image",
      "templateButton",
      "paragraph",
      "paragraph",
    ]);
    const inside = new Set([11, 12, 13, 14, 15, 16].map(id));
    for (const block of blocks) {
      if (inside.has(block.id)) {
        expect(block.inlines, block.kind).toEqual([]);
        expect(block.templateContent, block.kind).toBe(true);
      } else {
        expect(block.templateContent, block.kind).toBeUndefined();
      }
    }
    const byId = (n: number) => blocks.find((block) => block.id === id(n));
    expect(byId(1)?.inlines).toEqual([
      { type: "text", text: "Visible words " },
      { type: "pageRef", pageId: id(4) },
      // The date and the person read as their text inside the run around them.
      { type: "text", text: " 2026-10-07 14:30Grace" },
    ]);
    expect(byId(20)?.inlines).toEqual([{ type: "text", text: "After" }]);
    // The picture inside is not text, and still references its attachment.
    expect(byId(14)).toMatchObject({ kind: "image", inlines: [], attachmentId: id(200), caption: "hidden caption" });
    expect(extractAttachmentRefs(loaded(page()))).toEqual([{ blockId: id(14), attachmentId: id(200) }]);
  });

  it("list no link, date mention or person mention from inside a button", () => {
    const doc = loaded(page());
    expect(extractLinks(doc)).toEqual([{ fromBlock: id(1), toPage: id(4) }]);
    const dates = extractDateMentions(doc);
    expect(dates.map((mention) => [mention.nodeId, mention.date, mention.time, mention.reminderMinutes])).toEqual([
      [id(3), "2026-10-07", "14:30", 15],
    ]);
    expect(extractPersonMentions(doc)).toEqual([{ blockId: id(1), accountId: id(2), label: "Grace" }]);
  });
});

describe("the text of a page", () => {
  it("is Markdown without the button's content, with a mention's stored text whatever its `relative`", () => {
    const markdown = renderMarkdown(page(), noTitle);
    expect(markdown).not.toContain("Hidden");
    expect(markdown).not.toContain("Nested");
    expect(markdown).not.toContain("Add task");
    expect(markdown.startsWith("Visible words ")).toBe(true);
    expect(markdown.endsWith("\n\nAfter\n")).toBe(true);
    const relative = renderMarkdown(
      { type: "doc", content: [para(1, [text("Due "), date(2, "2026-10-18", { time: "09:30", relative: "now" }), text(" by "), person(1, "Ada", { relative: "me" })])] },
      noTitle,
    );
    expect(relative).toBe("Due 2026-10-18 09:30 by @Ada\n");
  });

  it("is HTML without the button's content", () => {
    const html = htmlPage(
      { title: "Plan", icon: undefined, properties: [], content: page() },
      {
        pageHref: () => undefined,
        attachmentHref: () => undefined,
        attachmentName: () => undefined,
        pageTitle: () => undefined,
        whiteboardHref: () => undefined,
      },
    );
    expect(html).toContain("Visible words");
    expect(html).toContain("After");
    expect(html).not.toContain("Hidden");
    expect(html).not.toContain("Nested");
    expect(html).not.toContain("hidden alt");
  });

  it("is what the AI index chunks: the blocks of the button have no text", () => {
    const blocks = blocksOfPortable(page(), noTitle);
    expect(blocks.filter((block) => block.text !== "").map((block) => block.id)).toEqual([id(1), id(20)]);
    expect(blocks.find((block) => block.id === id(11))?.text).toBe("");
    expect(blocks.find((block) => block.id === id(14))?.text).toBe("");
    expect(blocks.find((block) => block.id === id(20))?.text).toBe("After");
    const chunks = chunkPage("Plan", blocks);
    expect(chunks).toHaveLength(1);
    expect(chunks[0]?.text).toContain("Visible words");
    expect(chunks[0]?.text).not.toContain("Hidden");
    expect(chunks[0]?.text).not.toContain("hidden");
  });
});

describe("the in-memory core", () => {
  const OCT_7_1430 = Date.UTC(2026, 9, 7, 14, 30) / 1000;

  /** The wall clock reading `seconds` as an instant on this machine, as `now` returns it. */
  function atLocal(seconds: number): number {
    const wall = new Date(seconds * 1000);
    return new Date(
      wall.getUTCFullYear(),
      wall.getUTCMonth(),
      wall.getUTCDate(),
      wall.getUTCHours(),
      wall.getUTCMinutes(),
      wall.getUTCSeconds(),
    ).getTime();
  }

  /** Device 1, space 2; pages in creation order: Plan 3, Outside 4, Inside 5, Pictures 6. */
  function setup() {
    let counter = 0;
    const transport = new MemoryTransport({
      newId: () => id(++counter),
      now: () => atLocal(OCT_7_1430 - 2 * 3600),
      spaces: [
        {
          name: "Personal",
          pages: [
            { title: "Plan", content: page() },
            { title: "Outside" },
            { title: "Inside" },
            { title: "Pictures", content: page({ imageOutside: true }) },
          ],
        },
      ],
    });
    return new CoreClient(transport);
  }

  it("does not find the words of a button, and finds the others", async () => {
    const client = setup();
    expect((await client.search("visible")).map((hit) => hit.title)).toContain("Plan");
    expect((await client.search("hidden")).map((hit) => hit.title)).toEqual([]);
    expect((await client.search("nested")).map((hit) => hit.title)).toEqual([]);
    expect((await client.search("after")).map((hit) => hit.title)).toContain("Plan");
  });

  it("has a backlink from the text and none from a button", async () => {
    const client = setup();
    expect((await client.backlinksGet(id(4))).map((link) => link.title).sort()).toEqual(["Pictures", "Plan"]);
    expect(await client.backlinksGet(id(5))).toEqual([]);
  });

  it("never reminds of a date inside a button", async () => {
    const client = setup();
    const due = await client.remindersDue(OCT_7_1430);
    expect(due.map((reminder) => [reminder.pageTitle, reminder.nodeId]).sort()).toEqual([
      ["Pictures", id(3)],
      ["Plan", id(3)],
    ]);
  });

  it("takes the first picture of a page from outside the button", async () => {
    const client = setup();
    const space = (await client.spaceList())[0]?.id ?? "";
    expect(await client.pageFirstImages(space, [id(3), id(6)])).toEqual([{ pageId: id(6), attachmentId: id(201) }]);
  });
});
