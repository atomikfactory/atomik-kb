// The writing assistant's panel (ADR-0037, `docs/architecture/ai.md` section 8.3): below the selection,
// an action menu (with choices for the tone and the language), a field for a request of your own, then
// the answer as it streams, and Replace, Insert below, Retry, Stop and Discard (Escape). It only shows
// what the controller holds; nothing is written to the page until Replace or Insert below.

import { useEffect, useId, useRef, useState } from "react";
import type { FormEvent, KeyboardEvent } from "react";

import { AiMarkdown } from "../../components/AiMarkdown";
import { FloatingPanel } from "../../components/FloatingPanel";
import type { CloseReason } from "../../components/FloatingPanel";
import { activeLocale, t } from "../../i18n";
import { WRITER_ACTIONS, WRITER_LANGUAGES, WRITER_TONES } from "./actions";
import type { WriterAction, WriterChoice } from "./actions";
import { useObservable } from "./observable";
import { MAX_LANGUAGE_CHARS, MAX_PROMPT_CHARS, MAX_TEXT_CHARS } from "./text";
import type { WriterController } from "./writer";

export function AiWriterPanel({ controller }: { controller: WriterController }) {
  const state = useObservable(controller.state);
  if (!state.open) {
    return null;
  }
  return <Panel controller={controller} />;
}

function Panel({ controller }: { controller: WriterController }) {
  const state = useObservable(controller.state);
  const panel = useRef<HTMLElement | null>(null);
  const running = state.phase === "running";

  // The field that had the focus goes when the answer replaces the menu: the panel keeps it, so that
  // Escape and the buttons stay at hand.
  useEffect(() => {
    const element = panel.current;
    if (element !== null && !element.contains(document.activeElement)) {
      element.focus();
    }
  }, [state.phase]);

  return (
    <FloatingPanel
      anchor={state.anchor}
      label={t("aiWriter.label")}
      role="dialog"
      className="ai-writer"
      onClose={(reason?: CloseReason) => {
        // A press elsewhere (the page, to read on) keeps the panel and its answer: Escape or Discard end it.
        if (reason !== "outside") {
          controller.discard();
        }
      }}
      initialFocus={(element) => {
        panel.current = element;
        return element.querySelector<HTMLElement>("input") ?? element;
      }}
    >
      {state.phase === "choose" ? (
        <ChooseView controller={controller} cursor={state.kind === "cursor" || state.kind === "page"} />
      ) : (
        <>
          <div className="ai-writer-head">
            <span className="ai-writer-title">
              {state.label === null ? "" : t(state.label)}
              {state.detail === "" ? "" : `: ${state.detail}`}
            </span>
            <span className="ai-writer-status muted" role="status">
              {running ? t("aiWriter.status.running") : state.phase === "stopped" ? t("aiWriter.status.stopped") : ""}
            </span>
          </div>
          <Preview text={state.text} running={running} empty={state.phase === "done" && state.text.trim() === ""} />
          {state.truncated && <p className="ai-writer-note muted">{t("aiWriter.truncated", { count: MAX_TEXT_CHARS.toLocaleString(activeLocale()) })}</p>}
          {state.error !== null && (
            <p className="ai-writer-error error-text" role="alert">
              {state.phase === "error" ? t("aiWriter.error", { message: state.error }) : state.error}
            </p>
          )}
          <div className="ai-writer-buttons">
            {running ? (
              <button type="button" className="button" onClick={() => { controller.stop(); }}>
                {t("aiWriter.stop")}
              </button>
            ) : (
              <>
                {state.canReplace && (
                  <button
                    type="button"
                    className="button button-primary"
                    disabled={state.text.trim() === ""}
                    onClick={() => {
                      controller.apply("replace");
                    }}
                  >
                    {t("aiWriter.replace")}
                  </button>
                )}
                <button
                  type="button"
                  className={state.canReplace ? "button" : "button button-primary"}
                  disabled={state.text.trim() === ""}
                  onClick={() => {
                    controller.apply("below");
                  }}
                >
                  {t("aiWriter.insertBelow")}
                </button>
                <button type="button" className="button" onClick={() => { controller.retry(); }}>
                  {t("aiWriter.retry")}
                </button>
              </>
            )}
            <button
              type="button"
              className="button ai-writer-discard"
              onClick={() => {
                controller.discard();
              }}
            >
              {t("aiWriter.discard")}
            </button>
          </div>
        </>
      )}
    </FloatingPanel>
  );
}

/** The streamed answer, drawn from its Markdown (never as HTML; links stay text). It only becomes blocks of the page once applied. */
function Preview({ text, running, empty }: { text: string; running: boolean; empty: boolean }) {
  return (
    <div className="ai-writer-preview" aria-label={t("aiWriter.preview")} aria-busy={running} tabIndex={0}>
      {empty ? (
        <span className="muted">{t("aiWriter.empty")}</span>
      ) : text === "" ? (
        <span className="muted">…</span>
      ) : (
        <AiMarkdown markdown={text} />
      )}
    </div>
  );
}

interface ChooseProps {
  controller: WriterController;
  /** No selection: the actions that need text are left out, the field asks for something to write. */
  cursor: boolean;
}

/** Up, Down, Home and End move between the items of the menu (and of the choice open under one). */
function moveInMenu(event: KeyboardEvent<HTMLElement>): void {
  const items = Array.from(event.currentTarget.querySelectorAll<HTMLElement>('[role="menuitem"]:not(:disabled)'));
  const current = items.findIndex((item) => item === document.activeElement);
  let next: HTMLElement | undefined;
  switch (event.key) {
    case "ArrowDown":
      next = items[(current + 1) % items.length];
      break;
    case "ArrowUp":
      next = items[current <= 0 ? items.length - 1 : current - 1];
      break;
    case "Home":
      next = items[0];
      break;
    case "End":
      next = items[items.length - 1];
      break;
    default:
      return;
  }
  event.preventDefault();
  next?.focus();
}

/** The field and the action menu. */
function ChooseView({ controller, cursor }: ChooseProps) {
  const [prompt, setPrompt] = useState("");
  const [expanded, setExpanded] = useState<"tone" | "language" | null>(null);
  const [other, setOther] = useState(false);
  const [language, setLanguage] = useState("");
  const promptId = useId();
  const actions = WRITER_ACTIONS.filter((action) => !cursor || action.needs === "any");

  function submit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const text = prompt.trim();
    if (text === "") {
      if (cursor) {
        controller.run({ action: "continue" });
      }
      return;
    }
    controller.run({ action: "custom", prompt: text });
  }

  function choose(choice: WriterChoice): void {
    controller.run(choice);
  }

  function toggle(action: WriterAction): void {
    if (action.choice === undefined) {
      choose({ action: action.id });
      return;
    }
    setOther(false);
    setExpanded((current) => (current === action.choice ? null : (action.choice ?? null)));
  }

  return (
    <>
      <form className="ai-writer-prompt" onSubmit={submit}>
        <label className="visually-hidden" htmlFor={promptId}>
          {cursor ? t("aiWriter.prompt.cursor") : t("aiWriter.prompt.selection")}
        </label>
        <input
          id={promptId}
          className="input"
          type="text"
          value={prompt}
          maxLength={MAX_PROMPT_CHARS}
          placeholder={cursor ? t("aiWriter.prompt.cursor") : t("aiWriter.prompt.selection")}
          autoComplete="off"
          onChange={(event) => {
            setPrompt(event.target.value);
          }}
        />
        <button type="submit" className="button button-primary" disabled={prompt.trim() === "" && !cursor}>
          {t("aiWriter.prompt.send")}
        </button>
      </form>
      <div className="ai-writer-menu" role="menu" aria-label={t("aiWriter.actions")} onKeyDown={moveInMenu}>
        {actions.map((action) => (
          <div key={action.id}>
            <button
              type="button"
              role="menuitem"
              className="menu-item ai-writer-item"
              aria-haspopup={action.choice === undefined ? undefined : "menu"}
              aria-expanded={action.choice === undefined ? undefined : expanded === action.choice}
              onClick={() => {
                toggle(action);
              }}
            >
              {t(action.label)}
              {action.choice !== undefined && (
                <span className="ai-writer-more" aria-hidden="true">
                  {expanded === action.choice ? "▾" : "▸"}
                </span>
              )}
            </button>
            {action.choice === "tone" && expanded === "tone" && (
              <div className="ai-writer-sub" role="menu" aria-label={t("aiWriter.action.tone")}>
                {WRITER_TONES.map((tone) => (
                  <button
                    key={tone.id}
                    type="button"
                    role="menuitem"
                    className="menu-item"
                    onClick={() => {
                      choose({ action: "tone", tone: tone.id });
                    }}
                  >
                    {t(tone.label)}
                  </button>
                ))}
              </div>
            )}
            {action.choice === "language" && expanded === "language" && (
              <div className="ai-writer-sub" role="menu" aria-label={t("aiWriter.action.translate")}>
                {WRITER_LANGUAGES.map((entry) => (
                  <button
                    key={entry.name}
                    type="button"
                    role="menuitem"
                    className="menu-item"
                    onClick={() => {
                      choose({ action: "translate", language: entry.name });
                    }}
                  >
                    {t(entry.label)}
                  </button>
                ))}
                {other ? (
                  <form
                    className="ai-writer-other"
                    onSubmit={(event) => {
                      event.preventDefault();
                      const name = language.trim();
                      if (name !== "") {
                        choose({ action: "translate", language: name });
                      }
                    }}
                  >
                    <label className="visually-hidden" htmlFor={`${promptId}-language`}>
                      {t("aiWriter.language.otherLabel")}
                    </label>
                    <input
                      id={`${promptId}-language`}
                      className="input"
                      type="text"
                      autoFocus
                      value={language}
                      maxLength={MAX_LANGUAGE_CHARS}
                      placeholder={t("aiWriter.language.otherPlaceholder")}
                      autoComplete="off"
                      onChange={(event) => {
                        setLanguage(event.target.value);
                      }}
                    />
                    <button type="submit" className="button" disabled={language.trim() === ""}>
                      {t("aiWriter.language.go")}
                    </button>
                  </form>
                ) : (
                  <button
                    type="button"
                    role="menuitem"
                    className="menu-item"
                    onClick={() => {
                      setOther(true);
                    }}
                  >
                    {t("aiWriter.language.other")}
                  </button>
                )}
              </div>
            )}
          </div>
        ))}
      </div>
    </>
  );
}
