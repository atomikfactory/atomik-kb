// The read-only page view of the web viewer (M13 slice 4, ADR-0051, `docs/development/milestone-13.md` 4.2): one page,
// shown the way the app shows it (cover, icon, title and the page editor, in the same markup so that `app.css` draws
// them), with nothing that writes. It is what `apps/viewer` renders, over a `CoreClient` whose transport is the viewer's:
// the page's document comes from `docs.open` (the publication's shallow snapshot), its pictures from `attachments.read`,
// and the people its mentions name from `PeopleMentionable`.
//
// The editor is `PageEditor` with `locked`, so every write path of the app stays shut; the store behind it is the app's,
// never started (no workspace, no polling), and made again for every `revision`: a shallow snapshot carries no history,
// so a newer one cannot be merged into the document that is shown, and the view starts over on it (keeping the scroll
// position). The space the page claims to be in is a shared one in which this "account" is a Viewer, which is what keeps
// the selection toolbar, the comment button and the block handle away.
//
// **Content policy** (the page of the viewer has no `'unsafe-inline'`): nothing here writes a `<style>` element or a
// `style` attribute from markup; the app's components set positions and sizes through React's `style` prop, which is
// CSSOM and allowed. A page that needs inline markup the policy refuses (the HTML KaTeX makes) shows without it.

import type { CoreClient, MembersInfo, PageId, SpaceId, SpaceInfo } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { useCallback, useEffect, useRef, useState } from "react";

import { PageCover } from "../components/PageCover";
import { PageIcon } from "../components/PageIcon";
import { PageEditor } from "../editor/PageEditor";
import { loadLanguage, t } from "../i18n";
import { createAppStore } from "../store";
import type { AppStore } from "../store";
import { StoreContext } from "../store-context";

/** What the view shows of the page besides its document: the part of a publication that is not text. */
export interface ReadOnlyPage {
  /** The page. */
  pageId: PageId;
  /** The space the viewer names for it (any id: nothing outside the viewer sees it). */
  spaceId: SpaceId;
  /** The page's title (empty: "Untitled"). */
  title: string;
  /** The icon string (an emoji, `icon:<name>[:<color>]`, `attachment:<id>`). */
  icon?: string;
  /** The cover string (`attachment:<id>`, `color:<name>`, `gradient:<name>`). */
  cover?: string;
  /** The cover picture's vertical position in percent. */
  coverY?: number;
}

export interface ReadOnlyPageViewProps {
  /** The client over the viewer's transport. */
  client: CoreClient;
  /** The page. */
  page: ReadOnlyPage;
  /**
   * Which publication is shown: when it changes, the document and the pictures are read again, from the transport.
   * (The scroll position of the page is kept.)
   */
  revision: number;
}

/** The row `useSpaceAccess` and the member list read: a shared space in which the viewer is a Viewer. */
function viewerSpace(spaceId: SpaceId, name: string): SpaceInfo {
  return { id: spaceId, name, createdAt: 0, pageCount: 1, shareState: "Shared", role: "viewer", memberCount: 1 };
}

/** The member list the viewer gives the space: nobody to show, no settings that matter. */
function viewerMembers(spaceId: SpaceId): MembersInfo {
  return {
    spaceId,
    feedId: "",
    myRole: "viewer",
    epoch: 0,
    pendingRekey: false,
    members: [],
    formerMembers: [],
    invites: [],
    settings: { trashRetentionDays: 0, editorsMayLink: false },
    disputes: [],
    refusedTotal: 0,
    serverFlagged: false,
  };
}

/** The app's store for one revision, never started: the space and its (empty) member list are all it is given. */
function viewerStore(client: CoreClient, page: ReadOnlyPage): AppStore {
  const store = createAppStore(client);
  store.setState({
    spaces: [viewerSpace(page.spaceId, page.title)],
    members: { [page.spaceId]: viewerMembers(page.spaceId) },
  });
  return store;
}

const nothing = (): void => undefined;
const nothingAsync = (): Promise<void> => Promise.resolve();

interface RevisionProps {
  client: CoreClient;
  page: ReadOnlyPage;
  /** Told when the editor of this revision is up (`true`), so the caller can put the scroll position back. */
  onShown: () => void;
}

/** One revision: its own store, its own sessions, its own editor. */
function Revision({ client, page, onShown }: RevisionProps) {
  const [store] = useState(() => viewerStore(client, page));
  const onEditor = useCallback(
    (editor: Editor | null) => {
      if (editor !== null) {
        onShown();
      }
    },
    [onShown],
  );
  const untitled = page.title === "";
  return (
    <StoreContext.Provider value={store}>
      <section className="page viewer-page" aria-labelledby="page-title">
        {page.cover !== undefined && (
          <PageCover
            cover={page.cover}
            coverY={page.coverY}
            readOnly
            onChange={nothing}
            onSavePosition={nothingAsync}
            onRemove={nothingAsync}
          />
        )}
        <header className="page-header">
          <h1 className="title page-title" id="page-title">
            {page.icon !== undefined && page.icon !== "" && <PageIcon icon={page.icon} className="page-title-icon" />}
            <span className={untitled ? "page-title-text is-untitled" : "page-title-text"}>
              {untitled ? t("page.untitled") : page.title}
            </span>
          </h1>
        </header>
        <PageEditor pageId={page.pageId} spaceId={page.spaceId} onEditor={onEditor} locked />
      </section>
    </StoreContext.Provider>
  );
}

/**
 * The page of a view link, read-only. Give it the client of the viewer's transport, the page's parts that are not
 * text and the number of the publication that is shown; it follows the browser's language (the UI's strings: the
 * editor's, "Untitled").
 */
export function ReadOnlyPageView({ client, page, revision }: ReadOnlyPageViewProps) {
  const [languageReady, setLanguageReady] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const scrolled = useRef(0);

  useEffect(() => {
    let live = true;
    loadLanguage("system")
      .catch(() => "en")
      .then(() => {
        if (live) {
          setLanguageReady(true);
        }
      })
      .catch(nothing);
    return () => {
      live = false;
    };
  }, []);

  // The view of a newer publication starts at the top; the person was reading somewhere: put them back.
  const restoreScroll = useCallback(() => {
    if (scroller.current !== null && scrolled.current > 0) {
      scroller.current.scrollTop = scrolled.current;
    }
  }, []);

  if (!languageReady) {
    return null;
  }
  return (
    <div className="content viewer-content">
      <div
        className="content-body viewer-body"
        ref={scroller}
        onScroll={(event) => {
          // A revision that is being replaced empties the container and the browser moves it to the top: that is not
          // the reader's doing (the container's height is gone), so the value of the last real scroll is kept.
          const top = event.currentTarget.scrollTop;
          if (top > 0 || event.currentTarget.scrollHeight > event.currentTarget.clientHeight) {
            scrolled.current = top;
          }
        }}
      >
        <Revision key={revision} client={client} page={page} onShown={restoreScroll} />
      </div>
    </div>
  );
}
