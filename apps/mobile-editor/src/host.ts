// The messages between the Android app and the editor (apps/android/ARCHITECTURE.md, "Contract 2: WebView bridge"), as types
// and guards. The transport (`@atomik-kb/core-client/android`) carries calls and core events; what is left is below.

import type { HostMessage } from "@atomik-kb/core-client/android";

export type Theme = "light" | "dark";

/** `{"init": ..}`: which page to show and how. */
export interface InitInfo {
  pageId: string;
  spaceId: string;
  theme: Theme;
  locale: string;
  readOnly: boolean;
  focus: "title" | "end" | null;
}

/** `{"insert": ..}`: text at the caret, or an attachment as a block after it. */
export type InsertRequest = { text: string } | { attachmentId: string; name: string; mime: string };

/** `{"dictation": ..}`: the microphone's state, with the words heard so far. */
export interface DictationInfo {
  state: "listening" | "idle";
  partial?: string;
}

export type IncomingMessage =
  | { kind: "init"; init: InitInfo }
  | { kind: "insert"; insert: InsertRequest }
  | { kind: "theme"; theme: Theme }
  | { kind: "dictation"; dictation: DictationInfo };

function record(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : null;
}

function theme(value: unknown): Theme | null {
  return value === "light" || value === "dark" ? value : null;
}

/** Reads one message of the app; `null` for anything this editor does not know. */
export function parseIncoming(message: HostMessage): IncomingMessage | null {
  const init = record(message.init);
  if (init !== null && typeof init.pageId === "string" && typeof init.spaceId === "string") {
    return {
      kind: "init",
      init: {
        pageId: init.pageId,
        spaceId: init.spaceId,
        theme: theme(init.theme) ?? (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"),
        locale: typeof init.locale === "string" ? init.locale : "en",
        readOnly: init.readOnly === true,
        focus: init.focus === "title" || init.focus === "end" ? init.focus : null,
      },
    };
  }
  const insert = record(message.insert);
  if (insert !== null) {
    if (typeof insert.text === "string") {
      return { kind: "insert", insert: { text: insert.text } };
    }
    if (typeof insert.attachmentId === "string") {
      return {
        kind: "insert",
        insert: {
          attachmentId: insert.attachmentId,
          name: typeof insert.name === "string" ? insert.name : "",
          mime: typeof insert.mime === "string" ? insert.mime : "",
        },
      };
    }
  }
  const chosen = theme(message.theme);
  if (chosen !== null) {
    return { kind: "theme", theme: chosen };
  }
  const dictation = record(message.dictation);
  if (dictation !== null && (dictation.state === "listening" || dictation.state === "idle")) {
    return {
      kind: "dictation",
      dictation: {
        state: dictation.state,
        ...(typeof dictation.partial === "string" ? { partial: dictation.partial } : {}),
      },
    };
  }
  return null;
}
