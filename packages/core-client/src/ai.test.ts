import { describe, expect, it } from "vitest";

import { DEFAULT_MCP_PORT, aiSettingsOf, citationParts, defaultModels, isMcpPort, mcpSettingsOf } from "./ai";
import type { AiModelInfo } from "./generated/AiModelInfo";

function model(name: string, ...capabilities: string[]): AiModelInfo {
  return { name, capabilities, size: 1 };
}

describe("aiSettingsOf", () => {
  it("is off, with the semantic index on, when nothing is stored or the value is not an object", () => {
    const off = { enabled: false, semanticIndex: true, think: false };
    expect(aiSettingsOf(null)).toEqual(off);
    expect(aiSettingsOf(undefined)).toEqual(off);
    expect(aiSettingsOf("yes")).toEqual(off);
    expect(aiSettingsOf(true)).toEqual(off);
    expect(aiSettingsOf([1, 2])).toEqual(off);
  });

  it("reads every field", () => {
    expect(
      aiSettingsOf({
        enabled: true,
        chatModel: "qwen3:8b",
        embeddingModel: "nomic-embed-text:latest",
        transcriptionModel: "r2t2",
        semanticIndex: false,
        think: true,
      }),
    ).toEqual({
      enabled: true,
      chatModel: "qwen3:8b",
      embeddingModel: "nomic-embed-text:latest",
      transcriptionModel: "r2t2",
      semanticIndex: false,
      think: true,
    });
  });

  it("only takes a literal true for the switches that default to off, and only false turns the index off", () => {
    expect(aiSettingsOf({ enabled: "true", think: 1, semanticIndex: 0 })).toEqual({
      enabled: false,
      semanticIndex: true,
      think: false,
    });
  });

  it("leaves out models that are empty, blank or not strings", () => {
    const settings = aiSettingsOf({ enabled: true, chatModel: "  ", embeddingModel: 7, transcriptionModel: null });
    expect(settings).toEqual({ enabled: true, semanticIndex: true, think: false });
    expect("chatModel" in settings).toBe(false);
    expect(aiSettingsOf({ chatModel: " qwen3:8b " }).chatModel).toBe("qwen3:8b");
  });
});

describe("mcpSettingsOf", () => {
  it("is off on the default port without a usable value", () => {
    expect(mcpSettingsOf(null)).toEqual({ enabled: false, port: DEFAULT_MCP_PORT });
    expect(mcpSettingsOf("on")).toEqual({ enabled: false, port: DEFAULT_MCP_PORT });
    expect(DEFAULT_MCP_PORT).toBe(47811);
  });

  it("reads the switch and a port from 1024 to 65535", () => {
    expect(mcpSettingsOf({ enabled: true, port: 50000 })).toEqual({ enabled: true, port: 50000 });
    expect(mcpSettingsOf({ enabled: true, port: 1024 }).port).toBe(1024);
    expect(mcpSettingsOf({ enabled: true, port: 65535 }).port).toBe(65535);
  });

  it("falls back to the default port for a port out of range or not a whole number", () => {
    for (const port of [80, 1023, 65536, 47811.5, "47811", null, -1]) {
      expect(mcpSettingsOf({ enabled: true, port: port as never }).port).toBe(DEFAULT_MCP_PORT);
    }
  });

  it("knows what a port is", () => {
    expect(isMcpPort(47811)).toBe(true);
    expect(isMcpPort(1023)).toBe(false);
    expect(isMcpPort("47811")).toBe(false);
  });
});

describe("defaultModels", () => {
  it("prefers qwen3:8b and nomic-embed-text when installed", () => {
    const models = [
      model("gemma4:12b", "completion", "vision"),
      model("qwen3:8b", "completion", "thinking"),
      model("mxbai-embed-large:latest", "embedding"),
      model("nomic-embed-text:latest", "embedding"),
    ];
    expect(defaultModels(models)).toEqual({ chatModel: "qwen3:8b", embeddingModel: "nomic-embed-text:latest" });
  });

  it("falls back to the first completion model that is not an embedding model, and the first embedding model", () => {
    const models = [
      model("bge-m3", "embedding"),
      model("weird", "completion", "embedding"),
      model("gemma4:12b", "completion", "audio"),
      model("llama3.2:3b", "completion"),
    ];
    expect(defaultModels(models)).toEqual({ chatModel: "gemma4:12b", embeddingModel: "bge-m3" });
  });

  it("matches a nomic-embed-text with any tag", () => {
    expect(defaultModels([model("other-embed", "embedding"), model("nomic-embed-text:v1.5", "embedding")])).toEqual({
      embeddingModel: "nomic-embed-text:v1.5",
    });
  });

  it("leaves out what no model can do, and never picks a transcription model", () => {
    expect(defaultModels([])).toEqual({});
    expect(defaultModels([model("r2t2", "audio")])).toEqual({});
    expect(defaultModels([model("nomic-embed-text", "embedding")])).toEqual({ embeddingModel: "nomic-embed-text" });
    expect(defaultModels([model("gemma4:12b", "completion", "audio")])).toEqual({ chatModel: "gemma4:12b" });
  });
});

describe("citationParts", () => {
  const text = (value: string) => ({ type: "text", text: value }) as const;
  const cite = (n: number) => ({ type: "cite", n }) as const;

  it("gives nothing for an empty text and one text part for a text without citations", () => {
    expect(citationParts("")).toEqual([]);
    expect(citationParts("Nothing to cite here.")).toEqual([text("Nothing to cite here.")]);
  });

  it("splits at citations", () => {
    expect(citationParts("The plan is due in May [1] and costs 5 [2].")).toEqual([
      text("The plan is due in May "),
      cite(1),
      text(" and costs 5 "),
      cite(2),
      text("."),
    ]);
  });

  it("reads adjacent citations and lists of them", () => {
    expect(citationParts("a [2][3] b [1, 4] c [5,6,7]")).toEqual([
      text("a "),
      cite(2),
      cite(3),
      text(" b "),
      cite(1),
      cite(4),
      text(" c "),
      cite(5),
      cite(6),
      cite(7),
    ]);
  });

  it("starts and ends with a citation without empty texts", () => {
    expect(citationParts("[1] first, last [2]")).toEqual([cite(1), text(" first, last "), cite(2)]);
    expect(citationParts("[1]")).toEqual([cite(1)]);
  });

  it("leaves other brackets as text", () => {
    for (const value of ["[a]", "[]", "[1a]", "[ 1 ]", "[1, ]", "[x] and [y]", "array[i] = 3", "[2024-10-13]", "[123]", "[0]", "[1"]) {
      expect(citationParts(value)).toEqual([text(value)]);
    }
  });

  it("does not take the text of a Markdown link for a citation", () => {
    expect(citationParts("see [1](https://example.com) and [2]")).toEqual([
      text("see [1](https://example.com) and "),
      cite(2),
    ]);
  });

  it("keeps the text around a zero citation together", () => {
    expect(citationParts("a [0] b [1] c")).toEqual([text("a [0] b "), cite(1), text(" c")]);
  });

  it("is not fooled by HTML: markup stays plain text for the caller to render as text", () => {
    expect(citationParts('<img src=x onerror=alert(1)> [1]')).toEqual([text("<img src=x onerror=alert(1)> "), cite(1)]);
  });
});
