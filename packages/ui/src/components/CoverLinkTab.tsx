// The Link tab of the cover picker (ADR-0040): the link of a picture, downloaded once by the shell with the
// rules of "Image from URL" (ADR-0028): an https link, checked here as it is typed (`linkProblem`) and again
// by the shell, which stores the picture as an ordinary attachment of the page's space and never keeps the
// link. The cover becomes `attachment:<id>`. The tab is offered only where the shell can download a link.

import type { SpaceId } from "@atomik-kb/core-client";
import { MAX_IMAGE_URL_CHARS } from "@atomik-kb/core-client";
import { useContext, useId, useState } from "react";

import { describeUrlError } from "../editor/attachments";
import { linkProblem } from "../editor/ImageUrlDialog";
import { t } from "../i18n";
import { imageCoverValue } from "../icons/cover-grammar";
import { StoreContext } from "../store-context";

export interface CoverLinkTabProps {
  /** The space of the page: where the downloaded picture is stored. */
  spaceId: SpaceId;
  busy: boolean;
  setBusy: (busy: boolean) => void;
  setError: (message: string | null) => void;
  /** Stores the cover (`attachment:<id>`) and closes the picker. */
  apply: (cover: string) => Promise<void>;
}

export function CoverLinkTab({ spaceId, busy, setBusy, setError, apply }: CoverLinkTabProps) {
  const store = useContext(StoreContext);
  const fieldId = useId();
  const hintId = useId();
  const errorId = useId();
  const [text, setText] = useState("");
  const [checked, setChecked] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const problem = linkProblem(text);
  const shown = problem !== null && (checked || text.trim() !== "") ? problem : null;

  async function submit(): Promise<void> {
    if (busy || store === null) {
      return;
    }
    if (problem !== null) {
      setChecked(true);
      return;
    }
    setError(null);
    setBusy(true);
    setDownloading(true);
    let attachmentId: string;
    try {
      attachmentId = (await store.getState().importAttachmentFromUrl(spaceId, text.trim())).attachmentId;
    } catch (failure) {
      setError(describeUrlError(failure));
      setBusy(false);
      setDownloading(false);
      return;
    }
    setDownloading(false);
    await apply(imageCoverValue(attachmentId));
  }

  return (
    <form
      className="cover-link"
      noValidate
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <p id={hintId} className="muted">
        {t("attachment.url.hint")}
      </p>
      <label htmlFor={fieldId}>{t("attachment.url.label")}</label>
      <div className="row">
        <input
          id={fieldId}
          className="input"
          type="text"
          inputMode="url"
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          placeholder={t("attachment.url.placeholder")}
          value={text}
          data-autofocus=""
          disabled={busy}
          aria-invalid={shown !== null}
          aria-describedby={shown === null ? hintId : `${hintId} ${errorId}`}
          onChange={(event) => {
            setText(event.target.value);
            setChecked(false);
          }}
        />
        <button type="submit" className="button" disabled={busy || text.trim() === ""}>
          {t("cover.link.use")}
        </button>
      </div>
      {shown !== null && (
        <p className="error-text" id={errorId} role="alert">
          {t(shown, { max: MAX_IMAGE_URL_CHARS })}
        </p>
      )}
      {downloading && (
        <p className="icon-upload-status" role="status">
          {t("attachment.url.downloading")}
        </p>
      )}
    </form>
  );
}
