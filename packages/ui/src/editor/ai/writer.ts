// The writing assistant's controller (ADR-0037 decision 3, `docs/architecture/ai.md` section 8.3): one
// per page editor. It holds what the panel shows and runs the actions: the input is a range of the page
// as Markdown, the answer streams into the panel (never into the page), and Replace or Insert below
// writes it in one transaction. Every run has its own request id; a new run, Discard and the page going
// away cancel the one in progress.

import type { AiGenerateInput } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import type { Transaction } from "@tiptap/pm/state";

import type { Anchor } from "../../components/FloatingPanel";
import { toCoreError } from "@atomik-kb/core-client";
import { newRequestId } from "./ai-client";
import type { AiEditorClient } from "./ai-client";
import { applyMarkdown } from "./apply";
import type { ApplyTarget } from "./apply";
import { labelOfChoice } from "./actions";
import type { WriterChoice } from "./actions";
import { Observable } from "./observable";
import { clampText, contextBefore, markdownOfRange } from "./selection-input";
import type { Resolvers } from "./selection-input";
import { cleanOutput, MAX_LANGUAGE_CHARS, MAX_PROMPT_CHARS, MAX_TEXT_CHARS } from "./text";
import { isCancelledError } from "./generate";
import { t } from "../../i18n";
import type { MessageKey } from "../../i18n";

/** What the panel was opened for. */
export type WriterRequest =
  /** The text selection (the selection toolbar's AI button). */
  | { kind: "selection" }
  /** Whole top-level blocks (the block menu's "Ask AI"): from the start of the first to the end of the last. */
  | { kind: "blocks"; from: number; to: number }
  /** The cursor (`/ai`): continue, or answer a prompt. */
  | { kind: "cursor" }
  /** The page up to the cursor (`/summarize`, `/action-items`): runs at once. */
  | { kind: "page"; action: "summarize" | "actionItems" };

export type WriterPhase = "choose" | "running" | "done" | "stopped" | "error";

export interface WriterState {
  open: boolean;
  anchor: Anchor;
  kind: WriterRequest["kind"];
  /** Replace is offered: there is a selection to take the place of. */
  canReplace: boolean;
  phase: WriterPhase;
  /** What the current run does (for the header), `null` before one. */
  label: MessageKey | null;
  /** The tone or language of the current run, shown next to the label. */
  detail: string;
  /** The answer so far, cleaned. */
  text: string;
  error: string | null;
  /** The text of the selection was longer than the shell takes and was cut. */
  truncated: boolean;
}

export interface WriterDeps {
  editor: Editor;
  client: () => AiEditorClient | null;
  /** The title of the page. */
  title: () => string;
  resolvers: Resolvers;
}

const CLOSED: WriterState = {
  open: false,
  anchor: { x: 8, y: 48 },
  kind: "cursor",
  canReplace: false,
  phase: "choose",
  label: null,
  detail: "",
  text: "",
  error: null,
  truncated: false,
};

/** Where an anchor under position `pos` of the page goes (the panel opens below the text). */
function anchorUnder(editor: Editor, pos: number, blocks: boolean): Anchor {
  try {
    if (blocks) {
      const element = editor.view.nodeDOM(pos);
      if (element instanceof HTMLElement) {
        const rect = element.getBoundingClientRect();
        if (rect.height > 0) {
          return { x: Math.max(8, rect.left), y: rect.bottom + 6 };
        }
      }
    }
    const coords = editor.view.coordsAtPos(pos);
    return { x: Math.max(8, coords.left), y: coords.bottom + 6 };
  } catch {
    return { x: 8, y: 48 }; // no layout (tests), or a position that just went away
  }
}

export class WriterController {
  readonly state = new Observable<WriterState>(CLOSED);
  readonly #deps: WriterDeps;
  /** The range the run is about, kept up to date through the page's changes. */
  #from = 0;
  #to = 0;
  /** The id of the run in progress (its events are the only ones taken), `null` between runs. */
  #current: string | null = null;
  #last: WriterChoice | null = null;
  #input: { text: string; context: string } = { text: "", context: "" };

  constructor(deps: WriterDeps) {
    this.#deps = deps;
  }

  /** Starts following the page's changes (to keep the range right); returns what ends it: it cancels the run and closes the panel. */
  attach(): () => void {
    const { editor } = this.#deps;
    editor.on("transaction", this.#onTransaction);
    return () => {
      this.#cancelRun();
      editor.off("transaction", this.#onTransaction);
      this.state.set(CLOSED);
    };
  }

  readonly #onTransaction = ({ transaction }: { transaction: Transaction }): void => {
    if (!this.state.get().open || !transaction.docChanged) {
      return;
    }
    this.#from = transaction.mapping.map(this.#from, -1);
    this.#to = transaction.mapping.map(this.#to, 1);
  };

  /** Opens the panel for `request`; `page` requests start their run at once. */
  open(request: WriterRequest): void {
    if (!this.#deps.editor.isEditable || this.#deps.client() === null) {
      return;
    }
    this.#cancelRun();
    const { editor } = this.#deps;
    const { selection } = editor.state;
    let canReplace = false;
    switch (request.kind) {
      case "selection":
        canReplace = !selection.empty;
        this.#from = selection.from;
        this.#to = selection.to;
        break;
      case "blocks":
        canReplace = request.to > request.from;
        this.#from = request.from;
        this.#to = request.to;
        break;
      case "cursor":
      case "page":
        this.#from = selection.to;
        this.#to = selection.to;
        break;
    }
    if ((request.kind === "selection" || request.kind === "blocks") && !canReplace) {
      return; // nothing selected: nothing to ask about
    }
    this.#last = null;
    this.#input = { text: "", context: "" };
    this.state.set({
      ...CLOSED,
      open: true,
      kind: request.kind,
      canReplace,
      anchor: anchorUnder(editor, this.#to, request.kind === "blocks"),
    });
    if (request.kind === "page") {
      this.run({ action: request.action });
    }
  }

  /** Runs `choice` on the range the panel was opened for. */
  run(choice: WriterChoice): void {
    const client = this.#deps.client();
    const current = this.state.get();
    if (!current.open || client === null) {
      return;
    }
    this.#cancelRun();
    this.#last = choice;
    const input = this.#build(choice, current.kind);
    if (input === null) {
      // Nothing to work on (an empty page above the cursor, say): no request is made.
      this.state.set({ ...current, phase: "error", label: labelOfChoice(choice), detail: "", text: "", error: t("aiWriter.nothingToAsk"), truncated: false });
      return;
    }
    const requestId = newRequestId();
    this.#current = requestId;
    this.state.set({
      ...current,
      phase: "running",
      label: labelOfChoice(choice),
      detail: choice.tone !== undefined ? choice.tone : (choice.language ?? ""),
      text: "",
      error: null,
      truncated: this.#truncated,
    });
    let streamed = "";
    const take = (update: Partial<WriterState>): void => {
      if (this.#current === requestId) {
        this.state.update((state) => ({ ...state, ...update }));
      }
    };
    void client
      .aiGenerate(requestId, input, (event) => {
        if (this.#current !== requestId) {
          return;
        }
        switch (event.type) {
          case "delta":
            streamed += event.text;
            take({ text: cleanOutput(streamed) });
            break;
          case "done":
            take({ phase: "done", text: event.text.trim() === "" ? cleanOutput(streamed) : event.text.trim() });
            break;
          case "error":
            if (isCancelledError(event.error)) {
              take({ phase: "stopped" });
            } else {
              take({ phase: "error", error: event.error.message });
            }
            break;
          case "sources":
            break;
        }
      })
      .then(
        () => {
          // A stream that ended without saying so (a shell that sends no `done`) still gave its text.
          if (this.#current === requestId && this.state.get().phase === "running") {
            take({ phase: "done", text: cleanOutput(streamed) });
          }
        },
        (error: unknown) => {
          take({ phase: "error", error: toCoreError(error).message });
        },
      )
      .finally(() => {
        if (this.#current === requestId) {
          this.#current = null;
        }
      });
  }

  /** Runs the last choice again. */
  retry(): void {
    if (this.#last !== null) {
      this.run(this.#last);
    }
  }

  /** Stops the run in progress; what was written so far stays in the panel (and can still be used). */
  stop(): void {
    const requestId = this.#current;
    const client = this.#deps.client();
    if (requestId === null || client === null) {
      return;
    }
    client.aiCancel(requestId).catch(() => undefined);
    // The shell answers with a `cancelled` error; the panel does not wait for it.
    this.#current = null;
    this.state.update((state) => (state.phase === "running" ? { ...state, phase: "stopped" } : state));
  }

  /** Closes the panel and writes nothing (Escape, the Discard button). */
  discard(): void {
    this.#cancelRun();
    this.state.set(CLOSED);
  }

  /** Writes the answer: in place of what the panel was opened on, or after it. `true` when it was written. */
  apply(mode: "replace" | "below"): boolean {
    const state = this.state.get();
    if (!state.open || state.phase === "running" || state.text.trim() === "") {
      return false;
    }
    const { editor } = this.#deps;
    let target: ApplyTarget;
    if (mode === "replace") {
      if (!state.canReplace) {
        return false;
      }
      target =
        state.kind === "blocks"
          ? { mode: "replaceBlocks", from: this.#from, to: this.#to }
          : { mode: "replace", from: this.#from, to: this.#to };
    } else {
      target = { mode: "below", pos: this.#to, replaceEmpty: state.kind === "cursor" || state.kind === "page" };
    }
    if (!applyMarkdown(editor, state.text, target)) {
      this.state.update((value) => ({ ...value, error: t("aiWriter.applyFailed") }));
      return false;
    }
    this.discard();
    return true;
  }

  #cancelRun(): void {
    const requestId = this.#current;
    this.#current = null;
    if (requestId !== null) {
      this.#deps.client()?.aiCancel(requestId).catch(() => undefined);
    }
  }

  #truncated = false;

  /** The request for `choice` over the range the panel was opened for; `null` when there is nothing to send. */
  #build(choice: WriterChoice, kind: WriterRequest["kind"]): AiGenerateInput | null {
    const { editor, resolvers } = this.#deps;
    const title = this.#deps.title();
    const pageAction = choice.action === "summarize" || choice.action === "actionItems";
    let text = "";
    let context: string;
    if (kind === "selection" || kind === "blocks") {
      text = markdownOfRange(editor, this.#from, this.#to, resolvers);
      context = contextBefore(editor, this.#from, title, resolvers);
    } else if (pageAction) {
      // The page up to the cursor is the text; the title is all the context there is.
      text = markdownOfRange(editor, 0, this.#from, resolvers);
      context = title.trim() === "" ? "" : `Page title: ${title.trim()}`;
    } else {
      context = contextBefore(editor, this.#from, title, resolvers);
    }
    // Every action but a free request and the continuation of the page works on text.
    if (text.trim() === "" && choice.action !== "custom" && choice.action !== "continue") {
      return null;
    }
    const clamped = clampText(text, MAX_TEXT_CHARS);
    this.#truncated = clamped.truncated;
    this.#input = { text: clamped.text, context };
    const input: AiGenerateInput = { action: choice.action, text: clamped.text };
    if (context !== "") {
      input.context = context;
    }
    if (choice.tone !== undefined) {
      input.tone = choice.tone;
    }
    if (choice.language !== undefined) {
      input.language = choice.language.slice(0, MAX_LANGUAGE_CHARS);
    }
    if (choice.prompt !== undefined) {
      input.prompt = choice.prompt.slice(0, MAX_PROMPT_CHARS);
    }
    return input;
  }

  /** What the last run was sent (for tests and the preview's notice). */
  get lastInput(): { text: string; context: string } {
    return this.#input;
  }
}
