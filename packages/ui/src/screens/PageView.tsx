import { toCoreError } from "@atomik-kb/core-client";
import type { PageFont, PageInfo, SpaceId } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { useEffect, useState } from "react";

import { writeClipboard } from "../clipboard";
import { BacklinksPanel } from "../components/BacklinksPanel";
import { PagePath } from "../components/Breadcrumbs";
import { CoverPicker } from "../components/CoverPicker";
import { ExportDialog } from "../components/ExportDialog";
import { anchorBelow } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { IconPicker } from "../components/IconPicker";
import { CommentIcon, LockIcon, MoreIcon, StarFilledIcon, StarIcon } from "../components/icons";
import { ImportDialog } from "../components/ImportDialog";
import { InlineEdit } from "../components/InlineEdit";
import { Menu } from "../components/Menu";
import { MoveToSpaceDialog } from "../components/MoveToSpaceDialog";
import { PageCover } from "../components/PageCover";
import { PageHistoryDialog } from "../components/PageHistoryDialog";
import { PageIcon } from "../components/PageIcon";
import { PageInfoPanel } from "../components/PageInfoPanel";
import { PageAvatars } from "../components/PresenceAvatars";
import { PageLook, pageLookClasses } from "../components/PageLook";
import { PropertiesPanel } from "../components/PropertiesPanel";
import { TemplateBanner } from "../components/TemplateBanner";
import { TextDialog } from "../components/TextDialog";
import { ViewLinkDialog } from "../components/ViewLinkDialog";
import { blockLinkText } from "../editor/block-link";
import { requestFindInPage } from "../editor/FindBar";
import { PageEditor } from "../editor/PageEditor";
import { t, tp } from "../i18n";
import { usePrintMode } from "../print";
import { viewLinkRight } from "../sharing";
import { useSpaceAccess } from "../sharing-hooks";
import { MODIFIER } from "../shortcuts";
import { useAppStore, useStoreApi } from "../store-context";
import { selectCurrentCanEdit } from "../store-sharing";
import { selectFavorites, selectPageLocked } from "../store-structure";
import { templateRootOf } from "../template-tree";
import { locate } from "../tree";

interface Popup {
  anchor: Anchor;
  opener: Element;
}

/**
 * The open page: its cover banner (ADR-0040), the path above its title (the space and the pages above it,
 * ADR-0040), its icon and title (click the title to rename it), the page menu ("Copy as Markdown", "Copy link to page" (ADR-0040), "Export...", "Print...",
 * "Import into this page..." (ADR-0036), "Duplicate" (ADR-0040), "Move to..." and "Page info" (ADR-0040), and below
 * them the page font, small text and lock, ADR-0040), its properties, the editor and the backlinks. It also runs
 * print mode (`print.ts`) for the page: Ctrl+P, "Print..." and the Export dialog's PDF choice.
 *
 * The look of the page is classes on its section (`pageLookClasses`): the font and the small text apply to the
 * title, the properties and the text of this page only, never to the app around it, and print keeps them. A locked
 * page (ADR-0040; a guard against accidental edits, not a permission) shows "Locked" with an Unlock button in its
 * header and is read-only: its title cannot be renamed, its icon and cover cannot be changed, its properties and
 * its text cannot be edited. It follows the lock live, whichever device changes it. What the lock does not touch:
 * the commands of the page menu (export, import into the page, duplicate, move), the sidebar and every write that
 * is not made here.
 *
 * A page of a shared space follows the role (Milestone 13, ADR-0043) the way a lock does, as a permission and not as a guard: a
 * Viewer, a Commenter and everyone in a read-only copy get the read-only editor, title, icon, cover and
 * properties through the same read-only state of the editor, a "Read only" mark instead of the lock's Unlock button, and a page
 * menu without the commands that write (duplicate, import into the page, the font, small text and lock); moving a page out of a
 * shared space is an Owner's. Comments (ADR-0048) are the exception that a Commenter makes: the header's comments button (with the
 * count of open threads) opens the panel for everyone, the page menu's "Comment on this page" starts a page-level comment, and the
 * selection toolbar's "Comment" an anchored one.
 *
 * Templates (ADR-0056): a template page, and a page inside one, shows the template banner above its title (`TemplateBanner`: "Use
 * template" and "Stop using as template"); the page menu offers "Turn into template" for a page outside templates and "Stop using as
 * template" on a template page, to a role that can edit the space. The properties panel and the editor know the page is in a template
 * (placeholders, relative mentions).
 *
 * The page menu's "Page history" (ADR-0057) opens the history dialog (`PageHistoryDialog`) for every role: a Viewer looks at the versions and
 * their differences, an Editor restores one.
 *
 * The page menu's "Share a view link..." (ADR-0051, Milestone 13 slice 4) opens the view link dialog (`ViewLinkDialog`): offered to
 * an Owner, to an Editor when the space's "Editors may create view links" setting is on (disabled, with the reason, while it is off),
 * and in a personal space; not to a Commenter, a Viewer, a read-only copy or a page in the trash.
 */
export function PageView({ page }: { page: PageInfo }) {
  const renamePage = useAppStore((state) => state.renamePage);
  const setPageIcon = useAppStore((state) => state.setPageIcon);
  const setPageCover = useAppStore((state) => state.setPageCover);
  const setPageAppearance = useAppStore((state) => state.setPageAppearance);
  const store = useStoreApi();
  const loadBacklinks = useAppStore((state) => state.loadBacklinks);
  const exportMarkdown = useAppStore((state) => state.exportMarkdown);
  const titleFocus = useAppStore((state) => state.titleFocusPageId);
  const focusPageTitle = useAppStore((state) => state.focusPageTitle);
  const favorite = useAppStore((state) => selectFavorites(state).includes(page.id));
  const setPageFavorite = useAppStore((state) => state.setPageFavorite);
  const duplicatePage = useAppStore((state) => state.duplicatePage);
  const setPageTemplate = useAppStore((state) => state.setPageTemplate);
  // A template (ADR-0056) or a page inside one: "Turn into template" is for a page outside templates, "Stop using as template" for a template.
  const inTemplate = useAppStore((state) => templateRootOf(state.tree, page.id) !== null);
  // The menu bar's "Export..." (ADR-0040) asks for this page's Export dialog when a page is open.
  const exportRequested = useAppStore((state) => state.fileRequest?.target === "page" && state.fileRequest.kind === "export");
  const clearFileRequest = useAppStore((state) => state.clearFileRequest);
  const interop = useAppStore((state) => state.interopCapabilities);
  // "Move to..." is offered when there is another space to move to (ADR-0038).
  const access = useSpaceAccess(page.spaceId);
  // The setting that decides whether an Editor may create view links is in the member list of the space (read when the menu opens).
  const editorsMayLink = useAppStore((state) => state.members[page.spaceId]?.settings.editorsMayLink);
  const loadMembers = useAppStore((state) => state.loadMembers);
  const canMoveToSpace = useAppStore((state) => state.spaces.length > 1) && access.isOwner;
  const loadComments = useAppStore((state) => state.loadComments);
  const commentsOpen = useAppStore((state) => state.commentsOpen);
  const toggleComments = useAppStore((state) => state.toggleComments);
  const beginComment = useAppStore((state) => state.beginComment);
  const openThreads = useAppStore((state) =>
    state.pageComments?.pageId === page.id ? state.pageComments.threads.filter((thread) => !thread.resolved).length : 0,
  );
  const startPrint = useAppStore((state) => state.startPrint);
  const printing = useAppStore((state) => state.printing);
  const hasChildren = useAppStore((state) => {
    const found = state.tree === null ? null : locate(state.tree.roots, page.id);
    return found !== null && found.node.children.length > 0;
  });
  usePrintMode(page.title.trim() === "" ? t("page.untitled") : page.title);
  // A page made from the text of another (`/page`) opens with its title ready to be typed.
  const [renaming, setRenaming] = useState(titleFocus === page.id);
  const [iconPicker, setIconPicker] = useState<Popup | null>(null);
  const [coverPicker, setCoverPicker] = useState<Popup | null>(null);
  const [menu, setMenu] = useState<Popup | null>(null);
  // The "Page info" popover, and the editor it counts (told by the page editor once it is mounted).
  const [info, setInfo] = useState<Popup | null>(null);
  const [editor, setEditor] = useState<Editor | null>(null);
  const [markdown, setMarkdown] = useState<string | null>(null);
  // The Export, Import or Move dialog of the page menu, with the control that opened the menu (where focus returns).
  const [dialog, setDialog] = useState<{ kind: "export" | "import" | "move" | "viewLink" | "history"; returnTo: Element; space: SpaceId } | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const title = page.title === "" ? t("page.untitled") : page.title;
  const locked = page.locked === true;
  // The page is read-only for its lock (a guard) or for this account's role (a permission); both go through the same state.
  const readOnly = locked || !access.canEdit;
  // The cover can be changed on a live page, not in the trash, not on paper and not while the page is read-only.
  const coverReadOnly = page.trashedAt !== undefined || printing || readOnly;

  // A lock (or a role that stopped being allowed to edit) closes what edits the page (the rename field, the icon and cover
  // pickers) at once, whichever device did it (adjusting state while rendering: the effect-free way to follow a prop).
  const [lockedSeen, setLockedSeen] = useState(readOnly);
  if (lockedSeen !== readOnly) {
    setLockedSeen(readOnly);
    if (readOnly) {
      setRenaming(false);
      setIconPicker(null);
      setCoverPicker(null);
    }
  }

  // An Editor's right to create view links follows a setting of the space: read it when the menu opens, if it was not read yet.
  const needsSetting = menu !== null && access.role === "editor" && !access.archived && editorsMayLink === undefined;
  useEffect(() => {
    if (needsSetting) {
      // A refusal leaves the entry enabled: the core decides when it is used.
      loadMembers(page.spaceId).catch(() => undefined);
    }
  }, [needsSetting, loadMembers, page.spaceId]);

  // The request was taken when the view mounted (`renaming` above); it is not asked twice.
  useEffect(() => {
    if (titleFocus === page.id) {
      focusPageTitle(null);
    }
  }, [titleFocus, page.id, focusPageTitle]);

  useEffect(() => {
    loadBacklinks(page.id);
    return () => {
      loadBacklinks(null);
    };
  }, [loadBacklinks, page.id]);

  // The threads of the page: the header counts them, the panel lists them, the editor highlights them.
  useEffect(() => {
    loadComments(page.id);
    return () => {
      loadComments(null);
    };
  }, [loadComments, page.id]);

  function toggleFavorite(): void {
    setNotice(null);
    setPageFavorite(page.id, !favorite).catch((error: unknown) => {
      setNotice(t("favorites.error.set", { message: toCoreError(error).message }));
    });
  }

  /** Duplicates the page with its sub-pages; the store opens the copy, which replaces this page in the tab. */
  function duplicate(): void {
    setNotice(null);
    duplicatePage(page.id).catch((error: unknown) => {
      setNotice(t("page.duplicate.error", { message: toCoreError(error).message }));
    });
  }

  /** Makes the page a template, or a page again (ADR-0056); a refusal is said in the notice of the page. */
  function changeTemplate(template: boolean): void {
    setNotice(null);
    setPageTemplate(page.id, template).catch((error: unknown) => {
      setNotice(t("template.error.change", { message: toCoreError(error).message }));
    });
  }

  /** Changes the font, the small text or the lock of the page; a refusal is said in the notice of the page. */
  function changeLook(change: { font?: PageFont; smallText?: boolean; locked?: boolean }): void {
    setNotice(null);
    setPageAppearance(page.id, change).catch((error: unknown) => {
      setNotice(t("page.look.error", { message: toCoreError(error).message }));
    });
  }

  /** Runs a change of the cover; a refusal is said in the page's notice and rethrown for the banner to stay as it was. */
  async function changeCover(change: () => Promise<void>): Promise<void> {
    setNotice(null);
    try {
      await change();
    } catch (error) {
      setNotice(t("cover.error", { message: toCoreError(error).message }));
      throw error;
    }
  }

  /**
   * Writes the link to the page (`atomik-kb://page/<pageId>`, which the app registers with the system, ADR-0040) to the
   * clipboard and says so; where the clipboard cannot be written, shows the link to copy by hand.
   */
  async function copyLink(): Promise<void> {
    setNotice(null);
    const link = blockLinkText(page.id);
    try {
      await writeClipboard(link);
      setNotice(t("page.linkCopied"));
    } catch {
      setNotice(t("page.linkNoClipboard", { link }));
    }
  }

  async function copyMarkdown(): Promise<void> {
    setNotice(null);
    let text: string;
    try {
      text = await exportMarkdown(page.id);
    } catch (error) {
      setNotice(t("page.markdown.error", { message: toCoreError(error).message }));
      return;
    }
    try {
      await writeClipboard(text);
      setNotice(t("page.markdown.copied"));
    } catch {
      setMarkdown(text); // no clipboard here: show the text to copy by hand
    }
  }

  // A page in the trash cannot be linked.
  const linkRight = page.trashedAt === undefined ? viewLinkRight(access, editorsMayLink) : "none";
  const viewLinkItems =
    linkRight === "allowed"
      ? [{ id: "viewLink", label: t("viewLink.menu") }]
      : linkRight === "ownerOnly"
        ? [{ id: "viewLink", label: t("viewLink.menu"), disabled: true, title: t("viewLink.menu.ownerOnly") }]
        : [];

  return (
    <section className={pageLookClasses(page)} aria-labelledby="page-title">
      {page.cover !== undefined && (
        <PageCover
          key={`cover-${page.id}`}
          cover={page.cover}
          coverY={page.coverY}
          readOnly={coverReadOnly}
          onChange={(opener) => {
            setCoverPicker((current) => (current?.opener === opener ? null : { anchor: anchorBelow(opener), opener }));
          }}
          onSavePosition={(y) => changeCover(() => setPageCover(page.id, null, y))}
          onRemove={() => changeCover(() => setPageCover(page.id, null))}
        />
      )}
      <header className="page-header">
        <div className="page-header-top">
          <PagePath pageId={page.id} />
          {!access.canEdit && (
            <p
              className="page-lock role-lock"
              role="status"
              title={
                access.archived
                  ? t("sharing.archive.badge")
                  : t(access.role === "commenter" ? "sharing.readOnly.commenter" : "sharing.readOnly.viewer")
              }
            >
              <LockIcon />
              <span>{access.archived ? t("sharing.archive.badge") : t("sharing.readOnly.pill")}</span>
            </p>
          )}
          {locked && (
            <p className="page-lock" role="status">
              <LockIcon />
              <span>{t("page.locked")}</span>
              {access.canEdit && (
                <button
                  type="button"
                  className="link-button"
                  onClick={() => {
                    changeLook({ locked: false });
                  }}
                >
                  {t("page.unlock")}
                </button>
              )}
            </p>
          )}
          {access.shared && <PageAvatars spaceId={page.spaceId} pageId={page.id} />}
          <div className="page-header-actions">
            <button
              type="button"
              className="link-button page-icon-button"
              aria-haspopup="dialog"
              disabled={readOnly}
              title={locked ? t("page.locked.hint") : undefined}
              onClick={(event) => {
                const opener = event.currentTarget;
                setIconPicker((current) => (current?.opener === opener ? null : { anchor: anchorBelow(opener), opener }));
              }}
            >
              {page.icon === undefined ? t("page.addIcon") : t("page.changeIcon")}
            </button>
            {page.cover === undefined && !coverReadOnly && (
              <button
                type="button"
                className="link-button page-cover-button"
                aria-haspopup="dialog"
                onClick={(event) => {
                  const opener = event.currentTarget;
                  setCoverPicker((current) => (current?.opener === opener ? null : { anchor: anchorBelow(opener), opener }));
                }}
              >
                {t("page.addCover")}
              </button>
            )}
            <button
              type="button"
              className="icon-button page-comments-button"
              aria-pressed={commentsOpen}
              aria-label={openThreads === 0 ? t("comments.toggle") : tp("comments.toggleCount", openThreads)}
              title={t("comments.toggle")}
              onClick={toggleComments}
            >
              <CommentIcon />
              {openThreads > 0 && (
                <span className="page-comments-count" aria-hidden="true">
                  {openThreads}
                </span>
              )}
            </button>
            <button
              type="button"
              className="icon-button page-favorite"
              aria-pressed={favorite}
              aria-label={favorite ? t("favorites.remove") : t("favorites.add")}
              title={favorite ? t("favorites.remove") : t("favorites.add")}
              onClick={toggleFavorite}
            >
              {favorite ? <StarFilledIcon /> : <StarIcon />}
            </button>
            <button
              type="button"
              className="icon-button"
              aria-label={t("page.actions")}
              title={t("page.actions")}
              aria-haspopup="menu"
              onClick={(event) => {
                const opener = event.currentTarget;
                setInfo(null); // the info popover hangs from this button too
                setMenu((current) => (current?.opener === opener ? null : { anchor: anchorBelow(opener), opener }));
              }}
            >
              <MoreIcon />
            </button>
          </div>
        </div>
        <TemplateBanner page={page} />
        <h1 className="title page-title" id="page-title">
          {page.icon !== undefined && (
            <PageIcon icon={page.icon} className="page-title-icon" />
          )}
          {renaming && !readOnly ? (
            <InlineEdit
              className="page-title-edit"
              value={page.title}
              label={t("tree.renameLabel")}
              placeholder={t("page.untitled")}
              onCommit={async (next) => {
                // A lock that came while the field was open wins: the tree is read now, not as this render saw it.
                if (selectPageLocked(store.getState(), page.id) || !selectCurrentCanEdit(store.getState())) {
                  return;
                }
                try {
                  await renamePage(page.id, next);
                } catch (error) {
                  setNotice(t("tree.error.rename", { message: toCoreError(error).message }));
                }
              }}
              onDone={() => {
                setRenaming(false);
              }}
            />
          ) : readOnly ? (
            <span
              className={page.title === "" ? "page-title-text is-untitled" : "page-title-text"}
              title={locked ? t("page.locked.hint") : undefined}
            >
              {title}
            </span>
          ) : (
            <button
              type="button"
              className={page.title === "" ? "page-title-button is-untitled" : "page-title-button"}
              title={t("page.renameHint")}
              onClick={() => {
                setRenaming(true);
              }}
            >
              {title}
            </button>
          )}
        </h1>
        {notice !== null && (
          <p className="page-notice muted" role="status">
            {notice}
          </p>
        )}
        {printing && (
          <p className="page-notice muted" role="status">
            {t("print.preparing")}
          </p>
        )}
      </header>

      {/* Print mode, a locked page and a role that cannot edit show the properties as read-only values. */}
      <PropertiesPanel page={page} readOnly={printing || readOnly ? true : undefined} />
      <PageEditor key={page.id} pageId={page.id} spaceId={page.spaceId} onEditor={setEditor} locked={readOnly} />
      <BacklinksPanel pageId={page.id} />

      {menu !== null && (
        <Menu
          label={t("page.actions")}
          items={[
            { id: "find", label: t("page.find") },
            ...(access.canComment ? [{ id: "comment", label: t("comments.menu.page") }] : []),
            { id: "markdown", label: t("page.copyMarkdown") },
            { id: "copyLink", label: t("page.copyLink") },
            ...viewLinkItems,
            ...(interop.export ? [{ id: "export", label: t("interop.menu.export") }] : []),
            { id: "print", label: t("interop.menu.print"), hint: `${MODIFIER}+P` },
            ...(interop.import && access.canEdit ? [{ id: "import", label: t("interop.menu.importPage") }] : []),
            ...(access.canEdit ? [{ id: "duplicate", label: t("page.duplicate") }] : []),
            ...(access.canEdit && page.template === true ? [{ id: "stopTemplate", label: t("template.stopUsing") }] : []),
            ...(access.canEdit && !inTemplate && page.trashedAt === undefined
              ? [{ id: "turnIntoTemplate", label: t("template.turnInto") }]
              : []),
            ...(canMoveToSpace ? [{ id: "move", label: t("moveToSpace.menu") }] : []),
            { id: "history", label: t("history.dialog.menu") },
            { id: "info", label: t("page.info") },
          ]}
          anchor={menu.anchor}
          anchorElement={menu.opener}
          footer={access.canEdit ? <PageLook page={page} onChange={changeLook} /> : undefined}
          onSelect={(id) => {
            if (id === "find") {
              requestFindInPage();
            } else if (id === "comment") {
              beginComment({ pageId: page.id });
            } else if (id === "markdown") {
              void copyMarkdown();
            } else if (id === "copyLink") {
              void copyLink();
            } else if (id === "export" || id === "import" || id === "move" || id === "viewLink" || id === "history") {
              setDialog({ kind: id, returnTo: menu.opener, space: page.spaceId });
            } else if (id === "duplicate") {
              duplicate();
            } else if (id === "turnIntoTemplate" || id === "stopTemplate") {
              changeTemplate(id === "turnIntoTemplate");
            } else if (id === "print") {
              startPrint();
            } else if (id === "info") {
              setInfo({ anchor: menu.anchor, opener: menu.opener });
            }
          }}
          onClose={() => {
            setMenu(null);
          }}
        />
      )}
      {info !== null && (
        <PageInfoPanel
          page={page}
          editor={editor}
          anchor={info.anchor}
          anchorElement={info.opener}
          onClose={() => {
            setInfo(null);
          }}
        />
      )}
      {iconPicker !== null && (
        <IconPicker
          anchor={iconPicker.anchor}
          anchorElement={iconPicker.opener}
          current={page.icon}
          allowUpload
          spaceId={page.spaceId}
          onSelect={(icon) => setPageIcon(page.id, icon)}
          onClose={() => {
            setIconPicker(null);
          }}
        />
      )}
      {coverPicker !== null && (
        <CoverPicker
          anchor={coverPicker.anchor}
          anchorElement={coverPicker.opener}
          current={page.cover}
          spaceId={page.spaceId}
          onSelect={(cover) => setPageCover(page.id, cover)}
          onClose={() => {
            setCoverPicker(null);
          }}
        />
      )}
      {(dialog?.kind === "export" || exportRequested) && (
        <ExportDialog
          spaceId={page.spaceId}
          pageId={page.id}
          name={page.title}
          hasChildren={hasChildren}
          returnFocus={dialog?.kind === "export" ? dialog.returnTo : null}
          onClose={() => {
            setDialog(null);
            clearFileRequest();
          }}
        />
      )}
      {dialog?.kind === "import" && (
        <ImportDialog
          spaceId={page.spaceId}
          parentId={page.id}
          destination={page.title}
          returnFocus={dialog.returnTo}
          onClose={() => {
            setDialog(null);
          }}
        />
      )}
      {dialog?.kind === "move" && (
        <MoveToSpaceDialog
          pageId={page.id}
          title={page.title}
          spaceId={dialog.space}
          returnFocus={dialog.returnTo}
          onClose={() => {
            setDialog(null);
          }}
        />
      )}
      {dialog?.kind === "history" && (
        <PageHistoryDialog
          key={`history-${page.id}`}
          page={page}
          returnFocus={dialog.returnTo}
          onClose={() => {
            setDialog(null);
          }}
        />
      )}
      {dialog?.kind === "viewLink" && (
        <ViewLinkDialog
          spaceId={page.spaceId}
          pageId={page.id}
          onClose={() => {
            setDialog(null);
          }}
        />
      )}
      {markdown !== null && (
        <TextDialog
          title={t("page.markdown.title")}
          body={t("page.markdown.body")}
          text={markdown}
          onClose={() => {
            setMarkdown(null);
          }}
        />
      )}
    </section>
  );
}
