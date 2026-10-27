import { CoreClient } from "@atomik-kb/core-client";
import type { Transport } from "@atomik-kb/core-client";
import { Fragment, useEffect, useLayoutEffect, useState } from "react";
import type { ReactNode } from "react";

import { applyAccent, selectAccent } from "./accent";
import { FatalErrorScreen } from "./screens/FatalErrorScreen";
import { LoadingScreen } from "./screens/LoadingScreen";
import { LockScreen } from "./screens/LockScreen";
import { MainLayout } from "./screens/MainLayout";
import { OnboardingScreen } from "./screens/OnboardingScreen";
import { RevokedScreen } from "./screens/RevokedScreen";
import type { Language } from "./i18n";
import { createAppStore, selectTheme } from "./store";
import type { Phase, Route } from "./store";
import { StoreContext, useAppStore } from "./store-context";

export interface AppProps {
  /**
   * How the UI reaches the core. Read once when the app mounts; give `<App>` a different
   * `key` to switch transports.
   */
  transport: Transport;
  /** The screen the app opens on once a workspace exists (Home by default; the dev page uses it). */
  initialRoute?: Route;
}

/**
 * Applies the theme, the accent and the language to the document root: `data-theme` is `light`, `dark` or
 * `system` (the stylesheet follows `prefers-color-scheme` for `system`), `data-accent` one of the palette's
 * names, absent for the default blue (ADR-0040), and `lang` the language of the strings (ADR-0040 item 13: the
 * browser hyphenates, spell-checks and picks fonts by it). All are layout effects: they run in the commit that
 * puts the app on the page, before the browser paints it, so no frame shows the default look.
 */
function useDocumentAttributes(language: Language | null): void {
  const theme = useAppStore(selectTheme);
  const accent = useAppStore(selectAccent);
  useLayoutEffect(() => {
    if (language === null) {
      return undefined;
    }
    const root = document.documentElement;
    const previous = root.lang;
    root.lang = language;
    return () => {
      root.lang = previous;
    };
  }, [language]);
  useLayoutEffect(() => {
    const root = document.documentElement;
    root.dataset.theme = theme;
    return () => {
      delete root.dataset.theme;
    };
  }, [theme]);
  useLayoutEffect(() => {
    const root = document.documentElement;
    applyAccent(root, accent);
    return () => {
      delete root.dataset.accent;
    };
  }, [accent]);
}

/** True for a drag that carries files from outside (not a page or block dragged in the app). */
function carriesFiles(event: DragEvent): boolean {
  return event.dataTransfer?.types.includes("Files") === true;
}

/**
 * Files dropped where nothing takes them (the sidebar, Settings) are refused: by default the
 * webview would open the file in place of the app. The page editor takes file drops itself
 * (it handles them before this, so they arrive here already handled).
 */
function useFileDropGuard(): void {
  useEffect(() => {
    const onDragOver = (event: DragEvent): void => {
      if (carriesFiles(event) && !event.defaultPrevented) {
        event.preventDefault();
        if (event.dataTransfer !== null) {
          event.dataTransfer.dropEffect = "none";
        }
      }
    };
    const onDrop = (event: DragEvent): void => {
      if (carriesFiles(event)) {
        event.preventDefault();
      }
    };
    window.addEventListener("dragover", onDragOver);
    window.addEventListener("drop", onDrop);
    return () => {
      window.removeEventListener("dragover", onDragOver);
      window.removeEventListener("drop", onDrop);
    };
  }, []);
}

function screenFor(phase: Phase, revoked: boolean): ReactNode {
  switch (phase) {
    case "loading":
      return <LoadingScreen />;
    case "fatal":
      return <FatalErrorScreen />;
    case "onboarding":
      return <OnboardingScreen />;
    case "locked":
      return <LockScreen />;
    case "ready":
      // A device removed from the account keeps its notes but leaves the app (ADR-0022).
      return revoked ? <RevokedScreen /> : <MainLayout />;
  }
}

function Screens() {
  const phase = useAppStore((state) => state.phase);
  const revoked = useAppStore((state) => state.security?.thisDeviceRevoked === true);
  const language = useAppStore((state) => state.language);
  useDocumentAttributes(language);
  useFileDropGuard();

  // The strings are plain function calls, so a change of language draws the whole tree again: the language is its
  // key. Nothing is drawn while the first language file loads (a system that does not speak English), so that no
  // frame is in English (ADR-0040 item 13).
  return language === null ? null : <Fragment key={language}>{screenFor(phase, revoked)}</Fragment>;
}

/** The whole UI: owns the store, boots it, and shows the screen the app state calls for. */
export function App({ transport, initialRoute }: AppProps) {
  const [store] = useState(() => createAppStore(new CoreClient(transport), { initialRoute }));

  useEffect(() => store.getState().start(), [store]);

  return (
    <StoreContext value={store}>
      <Screens />
    </StoreContext>
  );
}
