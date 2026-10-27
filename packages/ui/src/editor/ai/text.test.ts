// The text rules of the AI features that need no editor.

import { describe, expect, it } from "vitest";

import { t } from "../../i18n";
import {
  cleanOutput,
  formatTimestamp,
  recordingName,
  splitParts,
  transcriptBlocks,
  transcriptMarkdown,
  transcriptPlainText,
} from "./text";

const FENCE = "```";

describe("cleanOutput", () => {
  it("removes think blocks, also one that is still open", () => {
    expect(cleanOutput("<think>hmm</think>\n\nThe answer")).toBe("The answer");
    expect(cleanOutput("Start <think>one</think> middle <think>two</think> end")).toBe("Start  middle  end");
    expect(cleanOutput("<think>still thinking")).toBe("");
    expect(cleanOutput("Done <think>and now")).toBe("Done");
  });

  it("removes one fence that wraps the whole answer, and only that", () => {
    expect(cleanOutput(`${FENCE}markdown\n# Title\n\ntext\n${FENCE}`)).toBe("# Title\n\ntext");
    expect(cleanOutput(`${FENCE}\nplain\n${FENCE}`)).toBe("plain");
    // A fence inside the text stays: the answer is code, or has code in it.
    expect(cleanOutput(`Intro\n${FENCE}js\nlet x;\n${FENCE}`)).toBe(`Intro\n${FENCE}js\nlet x;\n${FENCE}`);
    expect(cleanOutput(`${FENCE}js\na\n${FENCE}\n\nand\n\n${FENCE}js\nb\n${FENCE}`)).toBe(
      `${FENCE}js\na\n${FENCE}\n\nand\n\n${FENCE}js\nb\n${FENCE}`,
    );
  });

  it("trims", () => {
    expect(cleanOutput("  \n text \n")).toBe("text");
    expect(cleanOutput("")).toBe("");
  });
});

describe("timestamps and transcripts", () => {
  it("formats mm:ss, and h:mm:ss from one hour", () => {
    expect(formatTimestamp(0)).toBe("00:00");
    expect(formatTimestamp(999)).toBe("00:00");
    expect(formatTimestamp(65_000)).toBe("01:05");
    expect(formatTimestamp(59 * 60_000 + 59_999)).toBe("59:59");
    expect(formatTimestamp(3_600_000)).toBe("1:00:00");
    expect(formatTimestamp(3_600_000 * 2 + 125_000)).toBe("2:02:05");
    expect(formatTimestamp(-5)).toBe("00:00");
  });

  const segments = [
    { startMs: 0, text: " Hello everyone. " },
    { startMs: 30_000, text: "" },
    { startMs: 60_000, text: "Let us start." },
  ];

  it("writes one `**[mm:ss]** text` paragraph per segment with words", () => {
    expect(transcriptMarkdown(segments)).toBe("**[00:00]** Hello everyone.\n\n**[01:00]** Let us start.");
    expect(transcriptMarkdown([])).toBe("");
    expect(transcriptPlainText(segments)).toBe("Hello everyone.\n\nLet us start.");
  });

  it("builds the paragraphs as nodes, so that what was said never becomes formatting", () => {
    const blocks = transcriptBlocks([{ startMs: 5_000, text: "**not bold** <details> # not a heading" }]);
    expect(blocks).toEqual([
      {
        type: "paragraph",
        content: [
          { type: "text", text: "[00:05]", marks: [{ type: "bold" }] },
          { type: "text", text: " **not bold** <details> # not a heading" },
        ],
      },
    ]);
  });
});

describe("splitParts", () => {
  it("keeps a text that fits in one part", () => {
    expect(splitParts("short", 100)).toEqual(["short"]);
  });

  it("splits at paragraph boundaries into parts of at most `max`", () => {
    const paragraph = "a".repeat(40);
    const parts = splitParts([paragraph, paragraph, paragraph, paragraph].join("\n\n"), 100);
    expect(parts).toEqual([`${paragraph}\n\n${paragraph}`, `${paragraph}\n\n${paragraph}`]);
  });

  it("cuts a paragraph longer than `max` at a space, or hard", () => {
    const words = Array.from({ length: 30 }, (_, index) => `word${String(index)}`).join(" ");
    const parts = splitParts(words, 50);
    expect(parts.length).toBeGreaterThan(1);
    for (const part of parts) {
      expect(part.length).toBeLessThanOrEqual(50);
    }
    expect(parts.join(" ")).toBe(words);
    expect(splitParts("x".repeat(250), 100).map((part) => part.length)).toEqual([100, 100, 50]);
  });
});

describe("recordingName", () => {
  it("names a recording `Recording YYYY-MM-DD HH.mm.webm` in local time", () => {
    expect(t("recorder.filePrefix")).toBe("Recording");
    expect(recordingName(new Date(2026, 9, 3, 7, 5))).toBe("Recording 2026-10-03 07.05.webm");
    expect(recordingName(new Date(2026, 11, 31, 23, 59, 30))).toBe("Recording 2026-12-31 23.59.webm");
  });
});
