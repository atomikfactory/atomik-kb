// The chat panel (ADR-0037, `docs/architecture/ai.md` section 8.4): a column to the right of the page where a
// question is answered from the notes. The scope selector picks where it looks (this space, all spaces, this
// page), the answer streams in with its sources first, `[n]` citations are chips that open the page, and each
// answer can be copied, saved as a page or inserted at the end of the open page. The conversation lives in the
// store (`store-ai.ts`) in memory only and is cleared when the app locks. With AI off the panel says so and
// points to Settings.

import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { FormEvent, KeyboardEvent } from "react";

import { t, tp } from "../i18n";
import { spaceAiOn } from "../sharing";
import { useCurrentAccess } from "../sharing-hooks";
import { useAppStore } from "../store-context";
import { MAX_QUESTION_CHARS, effectiveChatScope, selectAiEnabled, selectAiReady } from "../store-ai";
import type { ChatMessage, ChatScopeChoice } from "../store-ai";
import { selectCurrentSpace, selectPageLocked } from "../store-structure";
import { AiMarkdown } from "./AiMarkdown";
import { CloseIcon, PlusIcon } from "./icons";
import { PageIcon } from "./PageIcon";

/** The scroll position within this many pixels of the bottom counts as "at the bottom" (the log then follows the answer). */
const FOLLOW_PX = 80;

function Sources({ message }: { message: ChatMessage }) {
  const openChatSource = useAppStore((state) => state.openChatSource);
  if (message.sources.length === 0) {
    return null;
  }
  const count = message.sources.length;
  return (
    <details className="ai-sources">
      <summary>{tp("aiChat.sources", count)}</summary>
      <ol className="ai-source-list">
        {message.sources.map((source) => (
          <li key={source.n} value={source.n}>
            <button
              type="button"
              className="ai-source"
              onClick={() => {
                openChatSource(source);
              }}
            >
              <PageIcon icon={source.icon} className="ai-source-icon" />
              <span className="ai-source-title">{source.title === "" ? t("page.untitled") : source.title}</span>
            </button>
            <span className="ai-source-snippet muted">{source.text}</span>
          </li>
        ))}
      </ol>
    </details>
  );
}

function Answer({
  message,
  canInsert,
  locked,
  readOnlySpace,
}: {
  message: ChatMessage;
  canInsert: boolean;
  locked: boolean;
  readOnlySpace: boolean;
}) {
  const openChatSource = useAppStore((state) => state.openChatSource);
  const copyChatAnswer = useAppStore((state) => state.copyChatAnswer);
  const insertChatAnswer = useAppStore((state) => state.insertChatAnswer);
  const saveChatAnswerAsPage = useAppStore((state) => state.saveChatAnswerAsPage);
  const canSave = useAppStore((state) => state.currentSpaceId !== null);
  const streaming = message.status === "streaming";
  const finished = message.status === "done" || message.status === "stopped";
  return (
    <div className="ai-msg ai-msg-assistant" data-status={message.status}>
      {message.text === "" && streaming && (
        <p className="muted ai-thinking" role="status">
          {t("aiChat.thinking")}
        </p>
      )}
      {message.text !== "" && <AiMarkdown markdown={message.text} sources={message.sources} onCite={openChatSource} />}
      {message.status === "stopped" && <p className="muted">{t("aiChat.stopped")}</p>}
      {message.status === "error" && (
        <p className="error-text" role="alert">
          {message.error}
        </p>
      )}
      <Sources message={message} />
      {finished && message.text !== "" && (
        <div className="ai-msg-actions">
          <button
            type="button"
            className="button button-small"
            onClick={() => {
              void copyChatAnswer(message.id);
            }}
          >
            {t("aiChat.copy")}
          </button>
          <button
            type="button"
            className="button button-small"
            disabled={!canSave}
            onClick={() => {
              void saveChatAnswerAsPage(message.id);
            }}
          >
            {t("aiChat.save")}
          </button>
          <button
            type="button"
            className="button button-small"
            disabled={!canInsert}
            title={
              canInsert
                ? undefined
                : readOnlySpace
                  ? t("sharing.ai.insertReadOnly")
                  : locked
                    ? t("aiChat.insertLocked")
                    : t("aiChat.insertNoPage")
            }
            onClick={() => {
              insertChatAnswer(message.id);
            }}
          >
            {t("aiChat.insert")}
          </button>
        </div>
      )}
    </div>
  );
}

/** The panel: the header, the scope, the conversation and the box to type in. Shown by `MainLayout` while the chat is open. */
export function AiChatPanel() {
  const enabled = useAppStore(selectAiEnabled);
  const modelReady = useAppStore(selectAiReady);
  const currentSpace = useAppStore(selectCurrentSpace);
  const access = useCurrentAccess();
  const setSpaceAi = useAppStore((state) => state.setSpaceAi);
  const messages = useAppStore((state) => state.chatMessages);
  const streaming = useAppStore((state) => state.chatStreaming);
  const scope = useAppStore(effectiveChatScope);
  // A space whose AI is off (a shared one until this account opts in, Milestone 13) cannot be asked about; "All spaces" skips it.
  const spaceOff = !spaceAiOn(currentSpace) && scope !== "all";
  const ready = modelReady && !spaceOff;
  const hasPage = useAppStore((state) => state.selectedPageId !== null && state.route === "home");
  const askChat = useAppStore((state) => state.askChat);
  const stopChat = useAppStore((state) => state.stopChat);
  const newChat = useAppStore((state) => state.newChat);
  const closeChat = useAppStore((state) => state.closeChat);
  const setChatScope = useAppStore((state) => state.setChatScope);
  const navigate = useAppStore((state) => state.navigate);
  const [draft, setDraft] = useState("");
  const logRef = useRef<HTMLDivElement>(null);
  const boxRef = useRef<HTMLTextAreaElement>(null);
  const following = useRef(true);
  const scopeId = useId();
  const boxId = useId();

  // Typing goes into the box as soon as the panel opens.
  useEffect(() => {
    boxRef.current?.focus();
  }, [enabled]);

  // The log follows the answer while the person has not scrolled up.
  useLayoutEffect(() => {
    const log = logRef.current;
    if (log !== null && following.current) {
      log.scrollTop = log.scrollHeight;
    }
  }, [messages]);

  function onScroll(): void {
    const log = logRef.current;
    if (log !== null) {
      following.current = log.scrollHeight - log.scrollTop - log.clientHeight < FOLLOW_PX;
    }
  }

  function send(): void {
    const question = draft.trim();
    if (question === "" || streaming || !ready) {
      return;
    }
    following.current = true;
    setDraft("");
    void askChat(question);
  }

  function onSubmit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    send();
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): void {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      send();
    }
  }

  // "Insert into page" needs a page in front that is not locked (ADR-0040; the editor says whether it can take it).
  const locked = useAppStore((state) => hasPage && selectPageLocked(state, state.selectedPageId));
  const canInsert = hasPage && !locked && access.canEdit;

  return (
    <aside className="ai-chat" aria-label={t("aiChat.title")}>
      <header className="ai-chat-header">
        <h2 className="ai-chat-title">
          <span aria-hidden="true">✦ </span>
          {t("aiChat.title")}
        </h2>
        {enabled && (
          <button
            type="button"
            className="icon-button"
            aria-label={t("aiChat.new")}
            title={t("aiChat.new")}
            disabled={messages.length === 0}
            onClick={newChat}
          >
            <PlusIcon />
          </button>
        )}
        <button type="button" className="icon-button" aria-label={t("aiChat.close")} title={t("aiChat.close")} onClick={closeChat}>
          <CloseIcon />
        </button>
      </header>

      {!enabled ? (
        <div className="ai-chat-off">
          <p>{t("aiChat.off")}</p>
          <button
            type="button"
            className="button"
            onClick={() => {
              navigate("settings");
            }}
          >
            {t("aiChat.openSettings")}
          </button>
        </div>
      ) : (
        <>
          <div className="ai-chat-scope">
            <label htmlFor={scopeId}>{t("aiChat.scope")}</label>
            <select
              id={scopeId}
              className="input select"
              value={scope}
              onChange={(event) => {
                setChatScope(event.target.value as ChatScopeChoice);
              }}
            >
              <option value="space">{t("aiChat.scope.space")}</option>
              <option value="all">{t("aiChat.scope.all")}</option>
              {hasPage && <option value="page">{t("aiChat.scope.page")}</option>}
            </select>
          </div>

          <div ref={logRef} className="ai-chat-log" role="log" aria-label={t("aiChat.log")} aria-live="polite" onScroll={onScroll}>
            {messages.length === 0 ? (
              <p className="muted ai-chat-empty">{t("aiChat.empty")}</p>
            ) : (
              messages.map((message) =>
                message.role === "user" ? (
                  <div key={message.id} className="ai-msg ai-msg-user">
                    <p>{message.text}</p>
                  </div>
                ) : (
                  <Answer
                    key={message.id}
                    message={message}
                    canInsert={canInsert}
                    locked={locked}
                    readOnlySpace={!access.canEdit}
                  />
                ),
              )
            )}
          </div>

          <form className="ai-chat-composer" onSubmit={onSubmit}>
            {spaceOff && currentSpace !== null && (
              <div className="note ai-space-off" role="status">
                <p>{t("sharing.ai.off", { space: currentSpace.name })}</p>
                <button
                  type="button"
                  className="button"
                  onClick={() => {
                    void setSpaceAi(currentSpace.id, true).catch((error: unknown) => {
                      console.error("could not turn AI on for the space", error instanceof Error ? error.name : "error");
                    });
                  }}
                >
                  {t("sharing.ai.turnOn")}
                </button>
              </div>
            )}
            {!modelReady && <p className="note">{t("aiChat.noModel")}</p>}
            <label htmlFor={boxId} className="visually-hidden">
              {t("aiChat.placeholder")}
            </label>
            <textarea
              id={boxId}
              ref={boxRef}
              className="input ai-chat-box"
              rows={2}
              maxLength={MAX_QUESTION_CHARS}
              placeholder={t("aiChat.placeholder")}
              value={draft}
              disabled={!ready}
              onChange={(event) => {
                setDraft(event.target.value);
              }}
              onKeyDown={onKeyDown}
            />
            {streaming ? (
              <button type="button" className="button" onClick={stopChat}>
                {t("aiChat.stop")}
              </button>
            ) : (
              <button type="submit" className="button button-primary" disabled={!ready || draft.trim() === ""}>
                {t("aiChat.send")}
              </button>
            )}
          </form>
        </>
      )}
    </aside>
  );
}
