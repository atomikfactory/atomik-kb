// The writing assistant (ADR-0037, `docs/architecture/ai.md` section 8.3): entry points that show only
// while AI is on with a chat model, the panel (streamed preview, Replace, Insert below, Retry, Stop,
// Discard), the choices of tone and language, a request of your own, and the one transaction (one undo
// step) that writes an answer. Whole-app tests over the in-memory core with the fake AI.

import type { AiGenerateInput, JsonValue } from "@atomik-kb/core-client";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { t } from "../../i18n";
import { paragraphs, pid } from "../../test/editor";
import { openPage } from "../../test/page";
import { undo } from "../loro-binding";
import { AI_ON, outline, placeCursor, renderAiPages, resetAi, selectText } from "./ai-test-kit";

afterEach(() => {
  resetAi();
});

const space = (...texts: string[]) => [{ name: "Personal", pages: [{ title: "Notes", content: paragraphs(...texts) }] }];

/** The generate requests the fake AI got, in order. */
function requests(ai: { calls: readonly { kind: string; input: unknown }[] }): AiGenerateInput[] {
  return ai.calls.filter((call) => call.kind === "generate").map((call) => call.input as AiGenerateInput);
}

async function openNotes(texts: string[], options: Parameters<typeof renderAiPages>[1] = {}) {
  const app = await renderAiPages(space(...texts), options);
  const editor = await openPage(app.user, "Notes");
  return { ...app, editor };
}

/** Opens the panel from the selection toolbar for the characters `from` to `to`. */
async function askAboutSelection(app: Awaited<ReturnType<typeof openNotes>>, from: number, to: number) {
  selectText(app.editor, from, to);
  await app.user.click(await screen.findByRole("button", { name: t("aiWriter.buttonLabel") }));
  return screen.findByRole("dialog", { name: t("aiWriter.label") });
}

async function choose(app: Awaited<ReturnType<typeof openNotes>>, panel: HTMLElement, action: string) {
  await app.user.click(within(panel).getByRole("menuitem", { name: action }));
}

describe("when it is on offer", () => {
  it("shows no entry while AI is off, without a chat model, or when the shell has no AI", async () => {
    for (const aiSettings of [null, { ...AI_ON, enabled: false }, { ...AI_ON, chatModel: undefined }]) {
      const app = await openNotes(["Hello world"], { aiSettings });
      selectText(app.editor, 1, 6);
      await screen.findByRole("toolbar", { name: t("format.label") });
      expect(screen.queryByRole("button", { name: t("aiWriter.buttonLabel") })).toBeNull();
      app.unmount();
      resetAi();
    }
  });

  it("takes the client from the app's store when no test stands in for it", async () => {
    const app = await openNotes(["Hello world"]);
    resetAi(); // the store's own `aiClient`
    selectText(app.editor, 1, 6);
    expect(await screen.findByRole("button", { name: t("aiWriter.buttonLabel") })).toBeInTheDocument();
    await app.user.click(screen.getByRole("button", { name: t("aiWriter.buttonLabel") }));
    const panel = await screen.findByRole("dialog", { name: t("aiWriter.label") });
    await choose(app, panel, t("aiWriter.action.improve"));
    await within(panel).findByRole("button", { name: t("aiWriter.replace") });
    expect(requests(app.ai)).toHaveLength(1);
  });

  it("adds an AI button to the selection toolbar and an Ask AI item to the block menu", async () => {
    const app = await openNotes(["Hello world", "Second"]);
    selectText(app.editor, 1, 6);
    expect(await screen.findByRole("button", { name: t("aiWriter.buttonLabel") })).toBeInTheDocument();

    placeCursor(app.editor, 1);
    fireEvent.mouseMove(app.editor.view.dom.querySelector(`[data-id='${pid(0)}']`) as Element);
    await app.user.click(await screen.findByRole("button", { name: t("block.actions") }));
    expect(within(await screen.findByRole("menu")).getByRole("menuitem", { name: t("aiWriter.askBlock") })).toBeInTheDocument();
  });

  it("lists the AI commands of the / menu only while it is on", async () => {
    const { filterSlashCommands } = await import("../slash-commands");
    expect(filterSlashCommands("ai").map((command) => command.id)).toEqual(expect.arrayContaining(["ai", "aiSummarize", "aiActionItems"]));
    expect(filterSlashCommands("action-items")[0]?.id).toBe("aiActionItems");
    expect(filterSlashCommands("summarize")[0]?.id).toBe("aiSummarize");
    expect(filterSlashCommands("record")[0]?.id).toBe("record");

    const off = await openNotes([""], { aiSettings: null });
    placeCursor(off.editor, 1);
    act(() => {
      off.editor.commands.insertContent("/summarize");
    });
    await waitFor(() => {
      expect(screen.queryByRole("listbox", { name: t("slash.label") })).toBeNull();
    });
  });
});

describe("the panel", () => {
  it("streams the answer into the panel, writes nothing until Replace, and Replace is one undo step", async () => {
    const app = await openNotes(["hello world"]);
    const release = app.ai.holdStreams(1);
    const panel = await askAboutSelection(app, 1, 6);
    await choose(app, panel, t("aiWriter.action.improve"));

    // Streaming: Stop instead of the buttons, the answer so far in the preview, the page as it was.
    await within(panel).findByRole("button", { name: t("aiWriter.stop") });
    expect(within(panel).queryByRole("button", { name: t("aiWriter.replace") })).toBeNull();
    expect(app.editor.state.doc.textContent).toBe("hello world");
    expect(requests(app.ai)).toMatchObject([{ action: "improve", text: "hello" }]);

    release();
    await within(panel).findByRole("button", { name: t("aiWriter.replace") });
    expect(within(panel).getByLabelText(t("aiWriter.preview"))).toHaveTextContent("HELLO");
    expect(app.editor.state.doc.textContent).toBe("hello world");

    const before = (await import("../loro-binding")).loroSyncPluginKey.getState(app.editor.state)?.doc.oplogVersion();
    await app.user.click(within(panel).getByRole("button", { name: t("aiWriter.replace") }));
    expect(app.editor.state.doc.textContent).toBe("HELLO world");
    expect(screen.queryByRole("dialog", { name: t("aiWriter.label") })).toBeNull();
    const after = (await import("../loro-binding")).loroSyncPluginKey.getState(app.editor.state)?.doc;
    if (before !== undefined && after !== undefined) {
      expect(after.exportJsonUpdates(before).changes).toHaveLength(1);
    }

    undo(app.editor.state, app.editor.view.dispatch);
    expect(app.editor.state.doc.textContent).toBe("hello world");
  });

  it("inserts the answer as blocks below the selection's block", async () => {
    const app = await openNotes(["hello world", "Second"]);
    const panel = await askAboutSelection(app, 1, 6);
    await choose(app, panel, t("aiWriter.action.shorter"));
    await within(panel).findByRole("button", { name: t("aiWriter.insertBelow") });
    await app.user.click(within(panel).getByRole("button", { name: t("aiWriter.insertBelow") }));

    expect(outline(app.editor)).toEqual(["paragraph:hello world", "paragraph:hello", "paragraph:Second"]);
    undo(app.editor.state, app.editor.view.dispatch);
    expect(outline(app.editor)).toEqual(["paragraph:hello world", "paragraph:Second"]);
  });

  it("turns Markdown in the answer into blocks, in one transaction", async () => {
    const app = await openNotes(["a b"]);
    app.ai.setTranscriber(() => "");
    const panel = await askAboutSelection(app, 1, 4);
    // `actionItems` over "a b" answers a task list.
    await choose(app, panel, t("aiWriter.action.actionItems"));
    await app.user.click(await within(panel).findByRole("button", { name: t("aiWriter.insertBelow") }));
    expect(outline(app.editor)).toEqual(["paragraph:a b", "taskList:a b"]);
    undo(app.editor.state, app.editor.view.dispatch);
    expect(outline(app.editor)).toEqual(["paragraph:a b"]);
  });

  it("discards with Escape and with the button: nothing is written and the run is cancelled", async () => {
    const app = await openNotes(["hello world"]);
    const release = app.ai.holdStreams(1);
    const panel = await askAboutSelection(app, 1, 6);
    await choose(app, panel, t("aiWriter.action.improve"));
    await within(panel).findByRole("button", { name: t("aiWriter.stop") });

    fireEvent.keyDown(document.body, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: t("aiWriter.label") })).toBeNull();
    });
    release();
    expect(app.editor.state.doc.textContent).toBe("hello world");

    const second = await askAboutSelection(app, 1, 6);
    await choose(app, second, t("aiWriter.action.longer"));
    await app.user.click(await within(second).findByRole("button", { name: t("aiWriter.discard") }));
    expect(screen.queryByRole("dialog", { name: t("aiWriter.label") })).toBeNull();
    expect(app.editor.state.doc.textContent).toBe("hello world");
  });

  it("keeps the panel when the page is clicked: the answer is only ended by Escape or Discard", async () => {
    const app = await openNotes(["hello world"]);
    const panel = await askAboutSelection(app, 1, 6);
    await choose(app, panel, t("aiWriter.action.improve"));
    await within(panel).findByRole("button", { name: t("aiWriter.replace") });
    fireEvent.pointerDown(app.editor.view.dom);
    expect(screen.getByRole("dialog", { name: t("aiWriter.label") })).toBeInTheDocument();
  });

  it("stops a run and keeps what was written; Retry runs the action again", async () => {
    const app = await openNotes(["one two three four"]);
    const release = app.ai.holdStreams(1);
    const panel = await askAboutSelection(app, 1, 19);
    await choose(app, panel, t("aiWriter.action.shorter"));
    await app.user.click(await within(panel).findByRole("button", { name: t("aiWriter.stop") }));

    expect(within(panel).getByRole("status")).toHaveTextContent(t("aiWriter.status.stopped"));
    expect(within(panel).getByRole("button", { name: t("aiWriter.replace") })).toBeEnabled();
    release();

    await app.user.click(within(panel).getByRole("button", { name: t("aiWriter.retry") }));
    await waitFor(() => {
      expect(requests(app.ai)).toHaveLength(2);
    });
    await within(panel).findByRole("button", { name: t("aiWriter.replace") });
    expect(within(panel).getByLabelText(t("aiWriter.preview"))).toHaveTextContent("one two");
  });

  it("shows an error with Retry and discard, and writes nothing", async () => {
    const app = await openNotes(["hello"]);
    app.ai.failNextStream({ code: "InvalidInput", message: "the model is not installed", details: { field: "endpoint", reason: "model" } });
    const panel = await askAboutSelection(app, 1, 6);
    await choose(app, panel, t("aiWriter.action.improve"));
    expect(await within(panel).findByRole("alert")).toHaveTextContent("the model is not installed");
    expect(within(panel).getByRole("button", { name: t("aiWriter.retry") })).toBeInTheDocument();
    expect(app.editor.state.doc.textContent).toBe("hello");
  });
});

describe("what is asked", () => {
  it("sends the selection as Markdown, with the page title and the text before it as context", async () => {
    const page: JsonValue = {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Before the cut" }] },
        {
          type: "paragraph",
          attrs: { id: pid(1) },
          content: [
            { type: "text", text: "the " },
            { type: "text", text: "bold", marks: [{ type: "bold" }] },
            { type: "text", text: " idea" },
          ],
        },
      ],
    };
    const app = await renderAiPages([{ name: "Personal", pages: [{ title: "Notes", content: page }] }]);
    const editor = await openPage(app.user, "Notes");
    const second = editor.state.doc.child(0).nodeSize + 1;
    const panel = await askAboutSelection({ ...app, editor }, second, second + 13);
    await choose({ ...app, editor }, panel, t("aiWriter.action.improve"));
    await within(panel).findByRole("button", { name: t("aiWriter.replace") });

    const [request] = requests(app.ai);
    expect(request?.text).toBe("the **bold** idea");
    expect(request?.context).toContain("Page title: Notes");
    expect(request?.context).toContain("Before the cut");
    expect(request?.context).not.toContain("bold idea");
  });

  it("sends the tone, the language (also one typed) and a request of your own", async () => {
    const app = await openNotes(["hello"]);
    let panel = await askAboutSelection(app, 1, 6);
    await choose(app, panel, t("aiWriter.action.tone"));
    await app.user.click(within(panel).getByRole("menuitem", { name: t("aiWriter.tone.friendly") }));
    await within(panel).findByRole("button", { name: t("aiWriter.discard") });
    await app.user.click(within(panel).getByRole("button", { name: t("aiWriter.discard") }));

    panel = await askAboutSelection(app, 1, 6);
    await choose(app, panel, t("aiWriter.action.translate"));
    await app.user.click(within(panel).getByRole("menuitem", { name: t("aiWriter.language.french") }));
    await within(panel).findByRole("button", { name: t("aiWriter.discard") });
    await app.user.click(within(panel).getByRole("button", { name: t("aiWriter.discard") }));

    panel = await askAboutSelection(app, 1, 6);
    await choose(app, panel, t("aiWriter.action.translate"));
    await app.user.click(within(panel).getByRole("menuitem", { name: t("aiWriter.language.other") }));
    await app.user.type(within(panel).getByRole("textbox", { name: t("aiWriter.language.otherLabel") }), "Dutch{Enter}");
    await within(panel).findByRole("button", { name: t("aiWriter.discard") });
    await app.user.click(within(panel).getByRole("button", { name: t("aiWriter.discard") }));

    panel = await askAboutSelection(app, 1, 6);
    await app.user.type(within(panel).getByRole("textbox", { name: t("aiWriter.prompt.selection") }), "Make it rhyme{Enter}");
    await within(panel).findByRole("button", { name: t("aiWriter.replace") });

    expect(requests(app.ai).map((request) => [request.action, request.tone, request.language, request.prompt])).toEqual([
      ["tone", "friendly", undefined, undefined],
      ["translate", undefined, "French", undefined],
      ["translate", undefined, "Dutch", undefined],
      ["custom", undefined, undefined, "Make it rhyme"],
    ]);
  });

  it("cuts a selection longer than the shell takes, and says so", async () => {
    const long = "word ".repeat(7_000).trim();
    const app = await openNotes([long]);
    const panel = await askAboutSelection(app, 1, long.length + 1);
    await choose(app, panel, t("aiWriter.action.summarize"));
    await within(panel).findByRole("button", { name: t("aiWriter.insertBelow") });
    expect(requests(app.ai)[0]?.text).toHaveLength(32_000);
    expect(within(panel).getByText(t("aiWriter.truncated", { count: "32,000" }))).toBeInTheDocument();
  });
});

describe("the other entry points", () => {
  async function pickFromBlockMenu(app: Awaited<ReturnType<typeof openNotes>>, index: number) {
    placeCursor(app.editor, 1);
    fireEvent.mouseMove(app.editor.view.dom.querySelector(`[data-id='${pid(index)}']`) as Element);
    await app.user.click(await screen.findByRole("button", { name: t("block.actions") }));
    await app.user.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: t("aiWriter.askBlock") }));
    return screen.findByRole("dialog", { name: t("aiWriter.label") });
  }

  it("Ask AI in the block menu works on the whole block and Replace takes its place", async () => {
    const app = await openNotes(["first block", "second block"]);
    const panel = await pickFromBlockMenu(app, 0);
    await choose(app, panel, t("aiWriter.action.improve"));
    await app.user.click(await within(panel).findByRole("button", { name: t("aiWriter.replace") }));

    expect(requests(app.ai)[0]?.text).toBe("first block");
    expect(outline(app.editor)).toEqual(["paragraph:FIRST BLOCK", "paragraph:second block"]);
    undo(app.editor.state, app.editor.view.dispatch);
    expect(outline(app.editor)).toEqual(["paragraph:first block", "paragraph:second block"]);
  });

  async function runSlash(command: string, title: string, texts: string[] = [""]) {
    const app = await openNotes(texts);
    placeCursor(app.editor, app.editor.state.doc.content.size - 1);
    act(() => {
      app.editor.commands.insertContent(command);
    });
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(within(menu).getAllByRole("option")[0]?.querySelector(".suggestion-title")?.textContent).toBe(title);
    });
    return { ...app, menu };
  }

  it("/ai opens the panel at the cursor with a field; an empty request continues the text", async () => {
    const app = await runSlash("/ai", t("slash.ai"), ["Once upon a time", ""]);
    await app.user.click(within(app.menu).getByText(t("slash.ai")));
    const panel = await screen.findByRole("dialog", { name: t("aiWriter.label") });
    expect(app.editor.state.doc.textContent).toBe("Once upon a time");

    // At the cursor there is nothing to improve: only the actions that work without a selection.
    expect(within(panel).queryByRole("menuitem", { name: t("aiWriter.action.improve") })).toBeNull();
    expect(within(panel).queryByRole("button", { name: t("aiWriter.replace") })).toBeNull();
    await app.user.type(within(panel).getByRole("textbox", { name: t("aiWriter.prompt.cursor") }), "{Enter}");
    await within(panel).findByRole("button", { name: t("aiWriter.insertBelow") });

    const [request] = requests(app.ai);
    expect(request?.action).toBe("continue");
    expect(request?.text).toBe("");
    expect(request?.context).toContain("Once upon a time");
    await app.user.click(within(panel).getByRole("button", { name: t("aiWriter.insertBelow") }));
    // The empty line `/ai` was typed on is the answer's place.
    expect(outline(app.editor)).toEqual(["paragraph:Once upon a time", "paragraph:... and so on."]);
  });

  it("/ai with a request of your own asks for it with the page as context", async () => {
    const app = await runSlash("/ai", t("slash.ai"), ["Notes about cats", ""]);
    await app.user.click(within(app.menu).getByText(t("slash.ai")));
    const panel = await screen.findByRole("dialog", { name: t("aiWriter.label") });
    await app.user.type(within(panel).getByRole("textbox", { name: t("aiWriter.prompt.cursor") }), "Write a title{Enter}");
    await within(panel).findByRole("button", { name: t("aiWriter.insertBelow") });
    expect(requests(app.ai)[0]).toMatchObject({ action: "custom", prompt: "Write a title", text: "" });
  });

  it("/summarize runs at once on the page up to the cursor and inserts the summary", async () => {
    const app = await runSlash("/summarize", t("slash.aiSummarize"), ["First point", "Second point", ""]);
    await app.user.click(within(app.menu).getByText(t("slash.aiSummarize")));
    const panel = await screen.findByRole("dialog", { name: t("aiWriter.label") });
    await within(panel).findByRole("button", { name: t("aiWriter.insertBelow") });

    expect(requests(app.ai)).toHaveLength(1);
    expect(requests(app.ai)[0]).toMatchObject({ action: "summarize" });
    expect(requests(app.ai)[0]?.text).toBe("First point\n\nSecond point");
    expect(app.editor.state.doc.textContent).toBe("First pointSecond point");

    await app.user.click(within(panel).getByRole("button", { name: t("aiWriter.insertBelow") }));
    expect(outline(app.editor)).toEqual([
      "paragraph:First point",
      "paragraph:Second point",
      "paragraph:A short summary of the text.",
      "bulletList:Key point oneKey point two",
    ]);
  });

  it("/summarize on a page with no text above the cursor says so and asks nothing", async () => {
    const app = await runSlash("/summarize", t("slash.aiSummarize"), [""]);
    await app.user.click(within(app.menu).getByText(t("slash.aiSummarize")));
    const panel = await screen.findByRole("dialog", { name: t("aiWriter.label") });
    expect(await within(panel).findByRole("alert")).toHaveTextContent(t("aiWriter.nothingToAsk"));
    expect(requests(app.ai)).toEqual([]);
  });

  it("/action-items lists the to-dos of the page up to the cursor", async () => {
    const app = await runSlash("/action-items", t("slash.aiActionItems"), ["Call Ann", "Send the file", ""]);
    await app.user.click(within(app.menu).getByText(t("slash.aiActionItems")));
    const panel = await screen.findByRole("dialog", { name: t("aiWriter.label") });
    await within(panel).findByRole("button", { name: t("aiWriter.insertBelow") });
    await app.user.click(within(panel).getByRole("button", { name: t("aiWriter.insertBelow") }));
    expect(outline(app.editor)).toEqual(["paragraph:Call Ann", "paragraph:Send the file", "taskList:Call AnnSend the file"]);
  });
});

describe("the page going away", () => {
  it("cancels the run in progress when the editor is unmounted", async () => {
    const app = await openNotes(["hello world"]);
    app.ai.holdStreams(1);
    const panel = await askAboutSelection(app, 1, 6);
    await choose(app, panel, t("aiWriter.action.improve"));
    await within(panel).findByRole("button", { name: t("aiWriter.stop") });
    app.unmount();
    expect(screen.queryByRole("dialog", { name: t("aiWriter.label") })).toBeNull();
  });
});
