import { useMemo } from "react";

import { RecentPages } from "../components/RecentPages";
import { RecoveryKitBanner } from "../components/RecoveryKitBanner";
import { RotationBanner, StaleKitBanner } from "../components/DeviceBanners";
import { SpaceBanners } from "../components/SpaceBanners";
import { useCurrentAccess } from "../sharing-hooks";
import { SyncForkBanner } from "../components/SyncForkBanner";
import { t } from "../i18n";
import { useAppStore } from "../store-context";
import { currentEntry, selectActiveTab } from "../store-tabs";
import { useEntryScroll } from "../tab-scroll";
import { withoutTemplates } from "../template-tree";
import { findPage } from "../tree";
import { PageView } from "./PageView";

/**
 * The main area: the page of the active tab (title, editor, backlinks), or, when the tab shows
 * the Home of its space, or its page is not in the tree yet, the workspace name, a hint for what
 * to do next and the recently visited pages (ADR-0029). Only the front tab is mounted
 * (ADR-0034): another tab shows another page, and its scroll position is remembered and put back
 * when it is in front again. Above either, the banners: a key rotation
 * waiting for the passphrase, the warning while sync found that the server's history does
 * not match this device's, and the reminders to re-issue the kit after joining with it or
 * to create one while there is none, and what the space in use says (Milestone 13: the banner of a read-only copy, the
 * notices of refused changes). They start where the page's text does (ADR-0035).
 */
export function HomeScreen() {
  return (
    <>
      <div className="home-banners">
        <RotationBanner />
        <SyncForkBanner />
        <StaleKitBanner />
        <RecoveryKitBanner />
        <SpaceBanners />
      </div>
      <HomeContent />
    </>
  );
}

function HomeContent() {
  const workspace = useAppStore((state) => state.workspace);
  const tree = useAppStore((state) => state.tree);
  const spacesLoaded = useAppStore((state) => state.spacesLoaded);
  const hasSpaces = useAppStore((state) => state.spaces.length > 0);
  const canEdit = useCurrentAccess().canEdit;
  const selectedPageId = useAppStore((state) => state.selectedPageId);
  const entry = useAppStore((state) => {
    const tab = selectActiveTab(state);
    return tab === null ? null : currentEntry(tab);
  });
  // The active tab says what shows (its entry is the mirror of `selectedPageId`, which stands in
  // while there are no tabs). A page in the trash or purged is not in the live tree, and one of
  // another space is not in it until that space is loaded: the Home shows then.
  const pageId = entry === null ? selectedPageId : entry.pageId;
  const page = useMemo(
    () => (tree === null || pageId === null ? null : findPage(tree.roots, pageId)),
    [tree, pageId],
  );
  useEntryScroll(entry, pageId === null || page !== null);

  if (page !== null) {
    return <PageView key={page.id} page={page} />;
  }

  let hint = t("home.hint.select");
  if (spacesLoaded && !hasSpaces) {
    hint = t("home.hint.noSpaces");
  } else if (tree !== null && withoutTemplates(tree.roots).length === 0) {
    // "Use New page" is no advice for a role that cannot make pages (Milestone 13).
    hint = canEdit ? t("home.hint.emptySpace") : t("sharing.home.emptyReadOnly");
  }

  return (
    <section className="home" aria-labelledby="home-title">
      <h1 className="title" id="home-title">
        {workspace?.name ?? t("nav.home")}
      </h1>
      <p className="lead">{hint}</p>
      <p className="muted">{t("home.privacy")}</p>
      <RecentPages />
    </section>
  );
}
