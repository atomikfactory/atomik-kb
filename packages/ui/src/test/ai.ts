// Helpers for the tests of local AI and MCP (ADR-0037): a workspace to ask questions about, the AI settings that
// turn it on, and the hooks of the in-memory AI.

import type { JsonValue, MemoryAiTransport, MemoryTransport, SeedSpace } from "@atomik-kb/core-client";

import { paragraphs } from "./editor";

/** The settings keys of AI and MCP. */
export const AI_SETTING = "app.ai.settings";
export const MCP_SETTING = "app.mcp.settings";

/** AI on, with the models the in-memory Ollama lists. */
export const AI_ON: JsonValue = {
  enabled: true,
  chatModel: "qwen3:8b",
  embeddingModel: "nomic-embed-text:latest",
  transcriptionModel: "r2t2:latest",
  semanticIndex: true,
  think: false,
};

/** Two spaces with pages to find: the garden and the soup in Personal, the roadmap in Work. */
export const AI_SPACES: readonly SeedSpace[] = [
  {
    name: "Personal",
    pages: [
      { title: "Garden plans", content: paragraphs("Tomatoes need full sun and regular watering.", "Plant the beans after the last frost.") },
      { title: "Cooking", content: paragraphs("Tomato soup needs ripe tomatoes and basil.") },
      { title: "Journal" },
    ],
  },
  { name: "Work", pages: [{ title: "Roadmap", content: paragraphs("Ship the tomato tracker in spring.") }] },
];

/** The in-memory AI of a transport made with the `ai` option. */
export function aiOf(memory: MemoryTransport): MemoryAiTransport {
  if (memory.ai === undefined) {
    throw new Error("the transport has no AI: give it the `ai` option");
  }
  return memory.ai;
}
