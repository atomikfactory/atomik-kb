import { describe, expect, it } from "vitest";

import {
  blocksOfPortable,
  buildChat,
  buildGenerate,
  buildTranscription,
  chunkPage,
  cleanOutput,
  cleanTranscript,
  contextSize,
  dot,
  embeddingInput,
  fakeChatText,
  keywordPassages,
  normalized,
  queryWords,
  rrf,
  splitDeltas,
  toSources,
} from "./memory-ai-text";
import type { AiPageText } from "./memory-ai-text";
import type { AiSource } from "./generated/AiSource";
import type { JsonValue } from "./generated/serde_json/JsonValue";

function page(title: string, ...blocks: string[]): AiPageText {
  return {
    pageId: `p-${title}`,
    spaceId: "s1",
    title,
    blocks: blocks.map((text, index) => ({ id: `${title}-b${String(index)}`, text })),
  };
}

describe("cleanOutput", () => {
  it("removes think blocks, a fence around the whole answer, and surrounding space", () => {
    expect(cleanOutput("<think>hmm</think>\n\nThe answer.")).toBe("The answer.");
    expect(cleanOutput("<think>a</think>x<think>b</think>y")).toBe("xy");
    expect(cleanOutput("<think>never finished")).toBe("");
    expect(cleanOutput("```markdown\n# Title\n\ntext\n```")).toBe("# Title\n\ntext");
    expect(cleanOutput("```\nplain\n```")).toBe("plain");
    expect(cleanOutput("  hi  ")).toBe("hi");
  });

  it("keeps fences that are part of the answer", () => {
    const code = "Use this:\n\n```ts\nlet x = 1;\n```";
    expect(cleanOutput(code)).toBe(code);
    const two = "```\na\n```\n\ntext\n\n```\nb\n```";
    expect(cleanOutput(two)).toBe(two);
  });
});

describe("cleanTranscript", () => {
  it("reads an ASR header for the language and removes the closing tag", () => {
    expect(cleanTranscript("language English<asr_text>Hello there.</asr_text>")).toEqual({
      text: "Hello there.",
      language: "English",
    });
  });

  it("removes a leading label, quotes and fences, and collapses spaces", () => {
    expect(cleanTranscript("Transcript: hello   world").text).toBe("hello world");
    expect(cleanTranscript("Here is the transcript:\nhello").text).toBe("hello");
    expect(cleanTranscript('"quoted speech"').text).toBe("quoted speech");
    expect(cleanTranscript("```\nfenced words\n```").text).toBe("fenced words");
  });

  it("turns silence into empty text", () => {
    for (const raw of ["[silence]", "(no speech)", "", "   ", "[no speech]."]) {
      expect(cleanTranscript(raw).text).toBe("");
    }
    expect(cleanTranscript("<think>x</think>[silence]").text).toBe("");
  });
});

describe("prompts", () => {
  it("builds a writing request: the system prompt, then the instruction, context and text", () => {
    const request = buildGenerate({ action: "translate", text: "Hello", language: "French", context: "Greetings\nbefore it" });
    expect(request.messages[0]).toMatchObject({ role: "system" });
    expect(request.messages[0]?.content).toContain("writing assistant");
    const user = request.messages[1]?.content ?? "";
    expect(user).toContain("Translate into French.");
    expect(user).toContain("Greetings\nbefore it");
    expect(user.endsWith("Text:\nHello")).toBe(true);
    expect(request.temperature).toBe(0.2);
  });

  it("has the temperature of each action (section 3.2)", () => {
    const temperature = (action: Parameters<typeof buildGenerate>[0]["action"]) =>
      buildGenerate({ action, text: "x", prompt: "p", language: "L", tone: "casual" }).temperature;
    expect(temperature("continue")).toBe(0.7);
    expect(temperature("improve")).toBe(0.4);
    expect(temperature("fixGrammar")).toBe(0.1);
    expect(temperature("shorter")).toBe(0.3);
    expect(temperature("longer")).toBe(0.6);
    expect(temperature("summarize")).toBe(0.2);
    expect(temperature("actionItems")).toBe(0.1);
    expect(temperature("custom")).toBe(0.6);
    expect(temperature("meetingSummary")).toBe(0.2);
  });

  it("uses the custom prompt as the instruction, and leaves out an empty text", () => {
    const request = buildGenerate({ action: "custom", text: "", prompt: "Write a haiku", context: "About autumn" });
    const user = request.messages[1]?.content ?? "";
    expect(user.startsWith("Write a haiku")).toBe(true);
    expect(user).not.toContain("Text:");
  });

  it("lists the numbered sources in the chat's system prompt and keeps the newest 40 messages", () => {
    const sources: AiSource[] = [
      { n: 1, pageId: "p1", spaceId: "s", title: "Garden", text: "Tomatoes need sun.", score: 1 },
      { n: 2, pageId: "p2", spaceId: "s", title: "Soup", text: "Boil it.", score: 0.5 },
    ];
    const messages = Array.from({ length: 45 }, (_, index) => ({
      role: index % 2 === 0 ? ("user" as const) : ("assistant" as const),
      content: `m${String(index)}`,
    }));
    const request = buildChat({ messages, scope: { type: "all" } }, sources);
    expect(request.messages).toHaveLength(41);
    expect(request.messages[0]?.content).toContain("[1] Garden\nTomatoes need sun.");
    expect(request.messages[0]?.content).toContain("[2] Soup\nBoil it.");
    expect(request.messages[1]?.content).toBe("m5");
    expect(request.messages.at(-1)?.content).toBe("m44");
    expect(request.temperature).toBe(0.3);
  });

  it("asks for a transcription with the WAV as the one image, at temperature 0", () => {
    const request = buildTranscription("AAAA");
    expect(request.messages).toEqual([
      { role: "user", content: "Transcribe the speech in this audio clip verbatim. Output only the transcript.", images: ["AAAA"] },
    ]);
    expect(request.temperature).toBe(0);
  });

  it("sizes the context: a third of a token per character plus 1,024, in steps of 1,024, from 4,096 to the model's maximum", () => {
    expect(contextSize(0)).toBe(4096);
    expect(contextSize(9_000)).toBe(4096);
    expect(contextSize(9_300)).toBe(5120);
    expect(contextSize(1_000_000)).toBe(16_384);
    expect(contextSize(1_000_000, 8192)).toBe(8192);
  });

  it("adds the prefix a model family wants to what it embeds", () => {
    expect(embeddingInput("nomic-embed-text:latest", "hello", "document")).toBe("search_document: hello");
    expect(embeddingInput("nomic-embed-text", "hello", "query")).toBe("search_query: hello");
    expect(embeddingInput("qwen3-embedding:8b", "hello", "query")).toBe(
      "Instruct: Given a question, retrieve passages of the user's notes that answer it\nQuery: hello",
    );
    expect(embeddingInput("qwen3-embedding:8b", "hello", "document")).toBe("hello");
    expect(embeddingInput("bge-m3", "hello", "query")).toBe("hello");
  });
});

describe("fake answers", () => {
  it("cites the first and second source", () => {
    const sources: AiSource[] = [
      { n: 1, pageId: "p1", spaceId: "s", title: "Garden", text: "t", score: 1 },
      { n: 2, pageId: "p2", spaceId: "s", title: "Soup", text: "t", score: 1 },
    ];
    const answer = fakeChatText("tomatoes?", sources);
    expect(answer).toContain("**Garden**");
    expect(answer).toContain("[1]");
    expect(answer).toContain("Soup [2]");
    expect(fakeChatText("q", sources.slice(0, 1))).not.toContain("[2]");
    expect(fakeChatText("zebras", [])).toBe('I could not find anything about "zebras" in your notes.');
  });

  it("splits text into at most n pieces that join back to the text", () => {
    const text = "one two three four five six seven eight nine ten";
    for (const count of [1, 2, 4, 20]) {
      const pieces = splitDeltas(text, count);
      expect(pieces.join("")).toBe(text);
      expect(pieces.length).toBeLessThanOrEqual(count);
    }
    expect(splitDeltas(text, 4)).toHaveLength(4);
    expect(splitDeltas("", 4)).toEqual([]);
    expect(splitDeltas("  lead", 3).join("")).toBe("  lead");
  });
});

describe("chunkPage", () => {
  it("makes one chunk of a short page, starting with the title", () => {
    const chunks = chunkPage("Garden", [
      { id: "a", text: "Tomatoes." },
      { id: "b", text: "  " },
      { id: "c", text: "Beans." },
    ]);
    expect(chunks).toEqual([{ blockIds: ["a", "c"], text: "Garden\nTomatoes.\nBeans." }]);
  });

  it("starts a new chunk before the next block would pass 1,000 characters", () => {
    const blocks = ["a", "b", "c"].map((id) => ({ id, text: id.repeat(600) }));
    const chunks = chunkPage("T", blocks);
    expect(chunks.map((chunk) => chunk.blockIds)).toEqual([["a"], ["b"], ["c"]]);
    for (const chunk of chunks) {
      expect(chunk.text.startsWith("T\n")).toBe(true);
    }
  });

  it("splits a block longer than 1,500 characters at sentence ends, keeping the block id", () => {
    const sentence = "This is a sentence of some length. ";
    const chunks = chunkPage("T", [{ id: "long", text: sentence.repeat(80) }]);
    expect(chunks.length).toBeGreaterThan(2);
    for (const chunk of chunks) {
      expect(chunk.blockIds.every((id) => id === "long")).toBe(true);
      expect(chunk.text.length).toBeLessThanOrEqual(1_000 + 2);
    }
  });

  it("hard-cuts a long block without a break at 1,500 characters", () => {
    const chunks = chunkPage("T", [{ id: "x", text: "z".repeat(4_000) }]);
    expect(chunks.length).toBeGreaterThanOrEqual(3);
  });

  it("gives a page without body text one chunk, its title, and an untitled empty page none", () => {
    expect(chunkPage("Only a title", [{ id: "a", text: "" }])).toEqual([{ blockIds: [], text: "Only a title" }]);
    expect(chunkPage("", [])).toEqual([]);
    expect(chunkPage("T".repeat(300), []).at(0)?.text).toHaveLength(200);
  });

  it("keeps at most 400 chunks", () => {
    const blocks = Array.from({ length: 1000 }, (_, index) => ({ id: String(index), text: "w".repeat(900) }));
    expect(chunkPage("T", blocks)).toHaveLength(400);
  });
});

describe("keyword retrieval", () => {
  it("folds case and diacritics and drops words of two characters or fewer", () => {
    expect(queryWords("The Café, is IN Zürich!")).toEqual(["the", "cafe", "zurich"]);
    expect(queryWords("a b cd")).toEqual([]);
    expect(queryWords("tomato tomato")).toEqual(["tomato"]);
  });

  it("scores by the share of words a block holds, with a prefix match on its words", () => {
    const pages = [page("A", "Tomatoes and basil in the garden."), page("B", "Garden gnomes."), page("C", "Nothing here.")];
    const found = keywordPassages(pages, "tomato basil garden");
    expect(found.map((passage) => passage.page.title)).toEqual(["A", "B"]);
    expect(found[0]?.score ?? 0).toBeGreaterThan(found[1]?.score ?? 1);
  });

  it("lets the title count a little for its blocks", () => {
    const pages = [page("Soup", "Boil the water."), page("Misc", "Boil the water.")];
    const found = keywordPassages(pages, "soup boil");
    expect(found.map((passage) => passage.page.title)).toEqual(["Soup", "Misc"]);
  });

  it("breaks ties by document order and keeps three passages per page", () => {
    const found = keywordPassages([page("P", "tomato 1", "tomato 2", "tomato 3", "tomato 4")], "tomato");
    expect(found.map((passage) => passage.text)).toEqual(["tomato 1", "tomato 2", "tomato 3"]);
  });

  it("numbers sources from 1, cuts the text at 1,500 characters and maps the ids", () => {
    const withIcon: AiPageText = { ...page("Icon", "x".repeat(2000) + " tomato"), icon: "🍅" };
    const sources = toSources(keywordPassages([withIcon, page("Other", "tomato")], "xxxx tomato"), 5);
    expect(sources.map((source) => source.n)).toEqual([1, 2]);
    expect(sources[0]?.text).toHaveLength(1500);
    expect(sources[0]).toMatchObject({ pageId: "p-Icon", spaceId: "s1", title: "Icon", icon: "🍅", blockId: "Icon-b0" });
    expect(toSources(keywordPassages([page("Other", "tomato")], "tomato"), 5)[0]?.icon).toBeUndefined();
    expect(toSources(keywordPassages([withIcon, page("Other", "tomato")], "tomato"), 1)).toHaveLength(1);
  });
});

describe("blocksOfPortable", () => {
  it("reads the text of every block with an id, in order, nested blocks included", () => {
    const content: JsonValue = {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: "p1" }, content: [{ type: "text", text: "Hello " }, { type: "text", text: "world", marks: [{ type: "bold" }] }] },
        {
          type: "bulletList",
          content: [
            {
              type: "listItem",
              attrs: { id: "li" },
              content: [{ type: "paragraph", attrs: { id: "p2" }, content: [{ type: "text", text: "Item" }, { type: "hardBreak" }, { type: "text", text: "next" }] }],
            },
          ],
        },
        { type: "paragraph", attrs: { id: "p3" }, content: [{ type: "pageRef", attrs: { pageId: "x" } }, { type: "dateMention", attrs: { date: "2026-10-13", time: "09:00" } }] },
        { type: "image", attrs: { id: "img", caption: "A caption", alt: "alt text" } },
        { type: "paragraph", content: [{ type: "text", text: "no id" }] },
      ],
    };
    expect(blocksOfPortable(content, (id) => (id === "x" ? "Linked page" : undefined))).toEqual([
      { id: "p1", text: "Hello world" },
      { id: "li", text: "" },
      { id: "p2", text: "Item next" },
      { id: "p3", text: "Linked page2026-10-13 09:00" },
      { id: "img", text: "A caption alt text" },
    ]);
    expect(blocksOfPortable("nonsense", () => undefined)).toEqual([]);
  });
});

describe("vectors", () => {
  it("normalises, keeps a zero vector zero and takes dot products", () => {
    const unit = normalized([3, 4]);
    expect(unit[0]).toBeCloseTo(0.6);
    expect(unit[1]).toBeCloseTo(0.8);
    expect(Array.from(normalized([0, 0]))).toEqual([0, 0]);
    expect(dot(Float32Array.of(1, 2, 3), Float32Array.of(4, 5, 6))).toBe(32);
    expect(dot(normalized([1, 1]), normalized([1, 1]))).toBeCloseTo(1);
  });

  it("fuses ranked lists by reciprocal rank, equals by first appearance", () => {
    const fused = rrf([
      ["a", "b", "c"],
      ["c", "a", "d"],
    ]);
    expect(fused.map((entry) => entry.key)).toEqual(["a", "c", "b", "d"]);
    expect(fused[0]?.score).toBeCloseTo(1 / 61 + 1 / 62);
    expect(rrf([["x"], ["y"]]).map((entry) => entry.key)).toEqual(["x", "y"]);
    expect(rrf([])).toEqual([]);
  });
});
