// The banner above the title of a template page, and of a page inside one (ADR-0056, `docs/architecture/templates.md` section 6):
// "Template — new pages made from it start with this content", with "Use template" (makes a page from the template under the
// template's parent, the top level for a top-level template, and opens it) and "Stop using as template". On a page inside a
// template it names the template, and both buttons act on that template. A role that cannot edit the space only reads it.
//
// The peek of a database row shows it too (`PeekHost`, where a database's "New template" and "Edit" open a template): `compact`
// stacks it for the narrow side panel, and `onUse` takes the page that "Use template" made instead of opening it in the tab, so
// that the peek can show it as a row would.

import { toCoreError } from "@atomik-kb/core-client";
import type { PageInfo } from "@atomik-kb/core-client";
import { useState } from "react";

import { t } from "../i18n";
import { useSpaceAccess } from "../sharing-hooks";
import { useAppStore } from "../store-context";
import { templateRootOf } from "../template-tree";
import { TemplateIcon } from "./template-icons";

export interface TemplateBannerProps {
  page: PageInfo;
  /** A narrow place (the side peek): the text on a line of its own above the buttons. */
  compact?: boolean;
  /** Called with the page "Use template" made, which is then not opened in the tab; absent: it is opened there. */
  onUse?: (made: PageInfo) => void;
}

export function TemplateBanner({ page, compact = false, onUse }: TemplateBannerProps) {
  const root = useAppStore((state) => templateRootOf(state.tree, page.id));
  const canEdit = useSpaceAccess(page.spaceId).canEdit;
  const createFromTemplate = useAppStore((state) => state.createFromTemplate);
  const setPageTemplate = useAppStore((state) => state.setPageTemplate);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (root === null || page.trashedAt !== undefined) {
    return null;
  }
  const template = root;

  async function run(action: () => Promise<unknown>, errorKey: "template.error.use" | "template.error.change"): Promise<void> {
    setNotice(null);
    setBusy(true);
    try {
      await action();
    } catch (failure) {
      setNotice(t(errorKey, { message: toCoreError(failure).message }));
    } finally {
      setBusy(false);
    }
  }

  const name = template.title === "" ? t("page.untitled") : template.title;
  const inside = template.id !== page.id;
  return (
    <div className={compact ? "template-banner is-compact" : "template-banner"} role="region" aria-label={t("template.banner.label")}>
      <span className="template-banner-icon" aria-hidden="true">
        <TemplateIcon />
      </span>
      <p className="template-banner-text">
        {inside ? t("template.banner.inside", { title: name }) : t("template.banner.text")}
      </p>
      {canEdit && (
        <span className="template-banner-actions">
          <button
            type="button"
            className="button button-small"
            disabled={busy}
            onClick={() => {
              void run(async () => {
                const made = await createFromTemplate(template.id, {
                  ...(template.parentId === undefined ? {} : { parentId: template.parentId }),
                  ...(onUse === undefined ? {} : { open: false }),
                });
                onUse?.(made);
              }, "template.error.use");
            }}
          >
            {t("template.banner.use")}
          </button>
          <button
            type="button"
            className="link-button"
            disabled={busy}
            onClick={() => {
              void run(() => setPageTemplate(template.id, false), "template.error.change");
            }}
          >
            {t("template.stopUsing")}
          </button>
        </span>
      )}
      {notice !== null && (
        <p className="error-text template-banner-notice" role="alert">
          {notice}
        </p>
      )}
    </div>
  );
}
