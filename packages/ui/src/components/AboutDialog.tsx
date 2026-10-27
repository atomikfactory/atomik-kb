import { useId } from "react";

import { t } from "../i18n";
import { useAppStore } from "../store-context";
import { Modal } from "./Modal";

/**
 * "About atomik-kb" of the desktop shell's menu bar (ADR-0040): the app's name, its version (the core's, which is the
 * app's) and the line the Home screen says about where the notes are kept. Escape, the button and a click on the
 * backdrop close it.
 */
export function AboutDialog({ onClose }: { onClose: () => void }) {
  const status = useAppStore((state) => state.status);
  const titleId = useId();
  const bodyId = useId();
  return (
    <Modal labelledBy={titleId} describedBy={bodyId} onDismiss={onClose}>
      <h2 className="subtitle" id={titleId}>
        {t("about.title")}
      </h2>
      <p id={bodyId}>{t("home.privacy")}</p>
      {status !== null && (
        <dl className="facts">
          <dt>{t("app.name")}</dt>
          <dd>{`${t("about.version")} ${status.coreVersion}`}</dd>
        </dl>
      )}
      <div className="dialog-actions">
        <button type="button" className="button" onClick={onClose}>
          {t("dialog.close")}
        </button>
      </div>
    </Modal>
  );
}
