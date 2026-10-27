// The dialog of the `/` command "Image from URL" (ADR-0028): one field for the link of a picture.
// The link is checked here as it is typed (it must be an https address: the same rules the
// shell applies again before it downloads anything) so that a mistake shows at once; Enter
// inserts, Escape or Cancel gives up, and focus goes back to the page. The editor asks for the
// link through `ImageUrlDialogStore.ask()` (the `AttachmentService.askUrl` of the page editor)
// and gets the text typed, or `null`; the link is never kept anywhere else.

import { MAX_IMAGE_URL_CHARS, parseImageUrl } from "@atomik-kb/core-client";
import { useId, useRef, useState, useSyncExternalStore } from "react";

import { Modal } from "../components/Modal";
import { t } from "../i18n";
import type { MessageKey } from "../i18n";

/**
 * What stands between the editor and the dialog: whether the dialog is open and who is waiting
 * for its answer. One dialog at a time; asking again while it is open gives up the first ask.
 */
export class ImageUrlDialogStore {
  #resolve: ((url: string | null) => void) | null = null;
  #open = false;
  readonly #listeners = new Set<() => void>();

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  readonly isOpen = (): boolean => this.#open;

  /** Opens the dialog and resolves with the link typed, or `null` when it was cancelled or closed. */
  readonly ask = (): Promise<string | null> => {
    this.cancel();
    return new Promise((resolve) => {
      this.#resolve = resolve;
      this.#set(true);
    });
  };

  /** The dialog's answer: a link the person typed. */
  readonly submit = (url: string): void => {
    this.#finish(url);
  };

  /** The dialog's other answer: nothing (Escape, Cancel, a click on the backdrop, the page closing). */
  readonly cancel = (): void => {
    this.#finish(null);
  };

  #finish(answer: string | null): void {
    const resolve = this.#resolve;
    this.#resolve = null;
    this.#set(false);
    resolve?.(answer);
  }

  #set(open: boolean): void {
    if (this.#open === open) {
      return;
    }
    this.#open = open;
    for (const listener of [...this.#listeners]) {
      listener();
    }
  }
}

/** The message that says what is wrong with a link as typed, or `null` when the shell would take it. */
export function linkProblem(text: string): MessageKey | null {
  const trimmed = text.trim();
  if (trimmed === "") {
    return "attachment.url.error.empty";
  }
  try {
    parseImageUrl(trimmed);
    return null;
  } catch {
    // Which rule it broke, for the message.
  }
  if (Array.from(trimmed).length > MAX_IMAGE_URL_CHARS) {
    return "attachment.url.error.tooLong";
  }
  let url: URL;
  try {
    url = new URL(trimmed);
  } catch {
    return "attachment.url.error.invalid";
  }
  if (url.protocol !== "https:") {
    return "attachment.url.error.scheme";
  }
  return url.username !== "" || url.password !== "" ? "attachment.url.error.credentials" : "attachment.url.error.invalid";
}

/**
 * Whether the text already names a scheme other than `https` (`http://`, `file:`, `javascript:`):
 * a mistake that is clear as soon as it is typed, unlike `https:` itself on its way to a link.
 */
function namesOtherScheme(text: string): boolean {
  const scheme = /^([a-z][a-z0-9+.-]*):/iu.exec(text.trim())?.[1];
  return scheme !== undefined && scheme.toLowerCase() !== "https";
}

export interface ImageUrlFormProps {
  /** Called with the trimmed link once it passes the check. */
  onSubmit: (url: string) => void;
  onCancel: () => void;
}

/** The dialog itself, open. */
export function ImageUrlForm({ onSubmit, onCancel }: ImageUrlFormProps) {
  const titleId = useId();
  const hintId = useId();
  const fieldId = useId();
  const errorId = useId();
  const field = useRef<HTMLInputElement>(null);
  const [text, setText] = useState("");
  // A problem shows at once for a link that names another scheme, and for anything else once
  // the person has tried to insert it or left the field.
  const [checked, setChecked] = useState(false);
  const problem = linkProblem(text);
  const shown = problem !== null && (checked || (text.trim() !== "" && namesOtherScheme(text))) ? problem : null;

  return (
    <Modal labelledBy={titleId} describedBy={hintId} onDismiss={onCancel} className="image-url-dialog" initialFocus={field}>
      <h2 className="subtitle" id={titleId}>
        {t("attachment.url.title")}
      </h2>
      <form
        className="form"
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          if (problem !== null) {
            setChecked(true);
            field.current?.focus();
            return;
          }
          onSubmit(text.trim());
        }}
      >
        <p id={hintId} className="muted">
          {t("attachment.url.hint")}
        </p>
        <label htmlFor={fieldId}>{t("attachment.url.label")}</label>
        <input
          id={fieldId}
          ref={field}
          className="input"
          type="text"
          inputMode="url"
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          placeholder={t("attachment.url.placeholder")}
          value={text}
          aria-invalid={shown !== null}
          aria-describedby={shown === null ? undefined : errorId}
          onChange={(event) => {
            setText(event.target.value);
          }}
          onBlur={() => {
            setChecked(text.trim() !== "");
          }}
        />
        {shown !== null && (
          <p className="error-text" id={errorId} role="alert">
            {t(shown, { max: MAX_IMAGE_URL_CHARS })}
          </p>
        )}
        <div className="dialog-actions">
          <button type="button" className="button" onClick={onCancel}>
            {t("dialog.cancel")}
          </button>
          <button type="submit" className="button button-primary">
            {t("attachment.url.insert")}
          </button>
        </div>
      </form>
    </Modal>
  );
}

/** Shows the dialog while `store` says it is open. */
export function ImageUrlDialog({ store }: { store: ImageUrlDialogStore }) {
  const open = useSyncExternalStore(store.subscribe, store.isOpen);
  return open ? <ImageUrlForm onSubmit={store.submit} onCancel={store.cancel} /> : null;
}
