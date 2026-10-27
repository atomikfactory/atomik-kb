import type {
  ApiError,
  AppStatus,
  CoreClient,
  DeviceInfo,
  Event as CoreEvent,
  JsonValue,
  Workspace,
} from "@atomik-kb/core-client";
import { toCoreError } from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";
import { createStore } from "zustand/vanilla";

import { ACCENT_KEY } from "./accent";
import { clearWhiteboardStash } from "./editor/whiteboard-copy";
import { LANGUAGE_KEY, loadLanguage, parseLanguageChoice, resetLanguage, resolveLanguage } from "./i18n";
import type { Language, LanguageChoice } from "./i18n";
import { EMPTY_AI, createAiSlice } from "./store-ai";
import type { AiState } from "./store-ai";
import { EMPTY_ATTACHMENTS, createAttachmentsSlice } from "./store-attachments";
import type { AttachmentsState } from "./store-attachments";
import { EMPTY_COMMENTS, createCommentsSlice } from "./store-comments";
import type { CommentsState } from "./store-comments";
import { EMPTY_CONTENT, createContentSlice } from "./store-content";
import type { ContentState } from "./store-content";
import { EMPTY_DEVICES, createDevicesSlice } from "./store-devices";
import type { DevicesState } from "./store-devices";
import { EMPTY_DRIVE, createDriveSlice } from "./store-drive";
import type { DriveState } from "./store-drive";
import { createFeedSlice } from "./store-feed";
import type { FeedState } from "./store-feed";
import { createBulkSlice } from "./store-bulk";
import type { BulkState } from "./store-bulk";
import { createGallerySlice } from "./store-gallery";
import type { GalleryState } from "./store-gallery";
import { createHistorySlice } from "./store-history";
import type { HistoryState } from "./store-history";
import { EMPTY_INTEROP, createInteropSlice } from "./store-interop";
import type { InteropState } from "./store-interop";
import { createLayoutSlice } from "./store-layout";
import type { LayoutState } from "./store-layout";
import { EMPTY_PEEK, createPeekSlice } from "./store-peek";
import type { PeekState } from "./store-peek";
import { createShellSlice } from "./store-shell";
import type { ShellState } from "./store-shell";
import { EMPTY_LINKS, createLinksSlice } from "./store-links";
import type { LinksState } from "./store-links";
import { EMPTY_PRESENCE, createPresenceSlice } from "./store-presence";
import type { PresenceSliceState } from "./store-presence";
import { EMPTY_PROPERTIES, createPropertiesSlice } from "./store-properties";
import type { PropertiesState } from "./store-properties";
import { EMPTY_REMINDERS, createRemindersSlice } from "./store-reminders";
import type { RemindersState } from "./store-reminders";
import { AUTO_LOCK_KEY, EMPTY_SECURITY, createSecuritySlice } from "./store-security";
import type { SecurityState } from "./store-security";
import { EMPTY_SHARING, createSharingSlice } from "./store-sharing";
import type { SharingState } from "./store-sharing";
import { EMPTY_STRUCTURE, createStructureSlice } from "./store-structure";
import type { StructureState } from "./store-structure";
import { EMPTY_SYNC, createSyncSlice } from "./store-sync";
import type { SyncSliceState } from "./store-sync";
import { EMPTY_TABS, TABS_KEY, createTabsSlice, storedTabsValue } from "./store-tabs";
import type { TabsState } from "./store-tabs";
import { createViewsSlice } from "./store-views";
import type { ViewsState } from "./store-views";

/** Which top-level screen the app shows. */
export type Phase = "loading" | "fatal" | "onboarding" | "locked" | "ready";

/** Where the first run is: naming the workspace, then its recovery kit. */
export type OnboardingStep = "workspace" | "kit";

/** Screens reachable from the navigation once a workspace exists. */
export type Route = "home" | "settings" | "diagnostics";

export type Theme = "light" | "dark" | "system";

/** Setting that stores the theme (`ui.*` keys belong to the UI; see the core's key grammar). */
export const THEME_KEY = "ui.theme";

/** Reads a stored theme; anything unexpected (unset, wrong type, newer value) means "system". */
export function parseTheme(value: JsonValue | null | undefined): Theme {
  return value === "light" || value === "dark" || value === "system" ? value : "system";
}

/**
 * Everything the UI shows. The structure half (spaces, page tree, selection, collapsed
 * pages and the actions on them) is documented in `store-structure.ts`, the content half
 * (page documents, referenced pages, backlinks, search) in `store-content.ts`, and the
 * security part (recovery kit, app lock, devices, backups) in `store-security.ts`, the
 * attachments (what blocks show, uploads, Settings -> Data) in `store-attachments.ts`, and
 * sync (status, Settings -> Sync) in `store-sync.ts`, and devices (pairing, recovery
 * through the kit, removing a device, rotations) in `store-devices.ts`, and the reminders (the
 * in-app toasts of date mentions' reminders, and the timer of a core with no shell) in
 * `store-reminders.ts`, and the page properties (the schema of each space and the actions on it
 * and on a page's values, ADR-0026) in `store-properties.ts`, and the device-local layout (sidebar
 * width and visibility, the favourites section, the recent pages, ADR-0029) in `store-layout.ts`, and the
 * tabs of the main area (ADR-0034, which own `currentSpaceId` and `selectedPageId`) in `store-tabs.ts`, and
 * import and export (the runs and their progress, toasts, print mode, scheduled backups, ADR-0036) in
 * `store-interop.ts`, and the local AI and MCP (settings, status, the chat panel's conversation, the search
 * dialog's related results, ADR-0037) in `store-ai.ts`, and the links of the operating system that open a page
 * (ADR-0040) in `store-deep-links.ts`, and what the desktop shell tells the page (the quick capture, the tray's
 * lock, the state of the capture shortcut, the words the page gives the tray) in `store-shell.ts`, and shared spaces (the
 * member lists, the profile name, the invite session of both flows and what the core's events say happened to this
 * account in a shared space, Milestone 13) in `store-sharing.ts`, and comments (the threads of the open page and their
 * panel, the people of the spaces, the inbox of notifications and its toasts, Milestone 13 slice 2) in `store-comments.ts`, and
 * presence (who is where in the shared spaces, this device's presence going out, the other devices' frames coming in, Milestone 13
 * slice 3) in `store-presence.ts`, and view-only links (the links of a space, made, listed and revoked, never holding a URL
 * after the one answer that makes it, Milestone 13 slice 4) in `store-links.ts`, and the database views' device-local state (which
 * view of each block shows, ADR-0054) in `store-views.ts`, the peek of a database row (the page beside or over the page, ADR-0054) in
 * `store-peek.ts`, the gallery's lookup of the first image of pages (ADR-0054) in `store-gallery.ts`, the feed's lookup of the
 * excerpts of pages (ADR-0058) in `store-feed.ts`, the bulk edit of the rows of a database block (ADR-0058) in `store-bulk.ts`, and the calls of the page history dialog (ADR-0057) in `store-history.ts`, and the Google Drive target of the scheduled backups (the status, connecting, the own client, ADR-0059) in `store-drive.ts`.
 */
export interface AppState
  extends StructureState,
    ContentState,
    SecurityState,
    AttachmentsState,
    SyncSliceState,
    DevicesState,
    RemindersState,
    PropertiesState,
    LayoutState,
    TabsState,
    InteropState,
    AiState,
    SharingState,
    CommentsState,
    PresenceSliceState,
    LinksState,
    ViewsState,
    PeekState,
    GalleryState,
    BulkState,
    FeedState,
    DriveState,
    HistoryState,
    ShellState {
  phase: Phase;
  /** Why the app cannot run; set when `phase` is `"fatal"`. */
  fatalError: ApiError | null;
  /** The step of the first run; only meaningful while `phase` is `"onboarding"`. */
  onboardingStep: OnboardingStep;
  route: Route;
  status: AppStatus | null;
  workspace: Workspace | null;
  device: DeviceInfo | null;
  /** Settings read so far. `null` means the setting is not set; keys never read are absent. */
  settings: Readonly<Record<string, JsonValue | null>>;
  /**
   * The language the strings are in now (ADR-0040 item 13), which is the key of the app's tree: a change of it
   * draws every screen again. `null` only while the first language file loads (a system that speaks French:
   * nothing is drawn until it is there, so that no frame is in English). Follows the `ui.language` setting.
   */
  language: Language | null;

  /**
   * Subscribes to core events and boots the app. Returns the function that undoes it;
   * safe to call again afterwards (React StrictMode mounts, unmounts and mounts again).
   */
  start: () => () => void;
  /**
   * Waits for the core, then loads status, workspace, device, theme, accent, language and, with a workspace,
   * spaces, security status, sync status and the auto-lock delay (without one, the pairing
   * and recovery status, for a session under way). A locked core shows the lock screen.
   * Also the "try again" action, and what unlocking, restoring a backup, pairing and
   * recovering end with.
   */
  boot: () => Promise<void>;
  navigate: (route: Route) => void;
  /**
   * Creates the workspace, and a first space named `firstSpaceName` with it when given;
   * the onboarding then continues with the recovery kit (`finishOnboarding` ends it).
   * Rejects with a `CoreError` when the core refuses a name.
   */
  createWorkspace: (name: string, firstSpaceName?: string) => Promise<void>;
  /** Leaves the onboarding for the app (after the recovery kit step, or skipping it). */
  finishOnboarding: () => void;
  /** Writes a setting and updates the cache; rejects with a `CoreError` on failure. */
  setSetting: (key: string, value: JsonValue) => Promise<void>;
  /** Reads a setting into the cache; `SettingChanged` keeps it fresh afterwards. */
  loadSetting: (key: string) => Promise<void>;
  /** Renames this device; rejects with a `CoreError` when the core refuses the name. */
  renameDevice: (name: string) => Promise<void>;
  refreshStatus: () => Promise<void>;
}

export type AppStore = StoreApi<AppState>;

export const selectTheme = (state: AppState): Theme => parseTheme(state.settings[THEME_KEY]);

/** The language choice stored on this device: `system` (the default), `en`, `fr` or `es`. */
export const selectLanguageChoice = (state: AppState): LanguageChoice => parseLanguageChoice(state.settings[LANGUAGE_KEY]);

/** How often to ask the shell whether the core has finished opening. */
const CORE_POLL_MS = 100;

type CoreWait = { kind: "ready" } | { kind: "locked" } | { kind: "failed"; error: ApiError } | { kind: "stopped" };

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

/**
 * What stays in memory while the app is locked: nothing that came from the workspace. The
 * theme, the accent and the language stay so that the lock screen keeps its colors and its words, and the
 * route so that unlocking returns to the same screen.
 */
function lockedState(state: AppState): Partial<AppState> {
  const theme = state.settings[THEME_KEY];
  const accent = state.settings[ACCENT_KEY];
  const language = state.settings[LANGUAGE_KEY];
  return {
    ...EMPTY_STRUCTURE,
    ...EMPTY_CONTENT,
    ...EMPTY_SECURITY,
    ...EMPTY_ATTACHMENTS,
    ...EMPTY_SYNC,
    ...EMPTY_DEVICES,
    ...EMPTY_REMINDERS,
    ...EMPTY_PROPERTIES,
    ...EMPTY_TABS,
    ...EMPTY_INTEROP,
    ...EMPTY_AI,
    ...EMPTY_SHARING,
    ...EMPTY_COMMENTS,
    ...EMPTY_PRESENCE,
    ...EMPTY_LINKS,
    ...EMPTY_PEEK,
    ...EMPTY_DRIVE,
    phase: "locked",
    fatalError: null,
    onboardingStep: "workspace",
    status: null,
    workspace: null,
    device: null,
    settings: {
      ...(theme === undefined ? {} : { [THEME_KEY]: theme }),
      ...(accent === undefined ? {} : { [ACCENT_KEY]: accent }),
      ...(language === undefined ? {} : { [LANGUAGE_KEY]: language }),
    },
  };
}

/**
 * The single store of the app. One instance per `<App>`, created from a `CoreClient`;
 * it owns what the UI shows and keeps it fresh by re-reading the slice an event names.
 */
export function createAppStore(
  client: CoreClient,
  options: { corePollMs?: number; initialRoute?: Route } = {},
): AppStore {
  const pollMs = options.corePollMs ?? CORE_POLL_MS;
  // Bumped by every start() and stop(); work started under an older value gives up quietly.
  let epoch = 0;
  // Core calls of this UI that create the workspace (onboarding, restore) still running.
  let creatingWorkspace = 0;

  return createStore<AppState>()((set, get, api) => {
    const isCurrent = (value: number): boolean => value === epoch;

    // Until the setting is read the app speaks the system's language. English has no file: it is in use from
    // here on, which also drops the language an earlier app of this page left behind (the tests make many).
    const systemLanguage = resolveLanguage("system");
    if (systemLanguage === "en") {
      resetLanguage();
    }

    /** Runs a refresh triggered by an event; a failure leaves the old data in place. */
    function refreshQuietly(refresh: () => Promise<unknown>): void {
      refresh().catch((error: unknown) => {
        console.error("could not refresh after a core event", error);
      });
    }

    async function ownWorkspace<T>(work: () => Promise<T>): Promise<T> {
      creatingWorkspace += 1;
      try {
        return await work();
      } finally {
        creatingWorkspace -= 1;
      }
    }

    /**
     * Loads the language a `ui.language` choice stands for and makes it the app's, which draws every screen again
     * and gives the shell its words in that language (ADR-0040 item 13). A language that did not change does nothing.
     */
    async function applyLanguage(choice: LanguageChoice): Promise<void> {
      const before = get().language;
      const language = await loadLanguage(choice);
      if (language !== before) {
        set({ language });
        shell.sendLabels();
      }
    }

    function showLocked(): void {
      // Drawings copied for a paste are workspace content too (ADR-0033).
      clearWhiteboardStash();
      // The chat's stream ends with the core; what it still delivers must not come back (ADR-0037).
      ai.onLocked();
      drive.onLocked();
      set((state) => lockedState(state));
    }

    const structure = createStructureSlice(client, set, get, refreshQuietly);
    const content = createContentSlice(client, set, get, refreshQuietly);
    const security = createSecuritySlice(client, set, get, refreshQuietly, { ownWorkspace, locked: showLocked });
    const attachments = createAttachmentsSlice(client, set, get, refreshQuietly);
    const sync = createSyncSlice(client, set, get, refreshQuietly);
    const devices = createDevicesSlice(client, set, get, refreshQuietly);
    const reminders = createRemindersSlice(client, set, get);
    const properties = createPropertiesSlice(client, set, get, refreshQuietly);
    const layout = createLayoutSlice(client, set, get);
    const tabs = createTabsSlice(client, set, get, refreshQuietly, structure.loadCurrent);
    const interop = createInteropSlice(client, set, get);
    const ai = createAiSlice(client, set, get);
    const sharing = createSharingSlice(client, set, get, refreshQuietly);
    const comments = createCommentsSlice(client, set, get, refreshQuietly);
    const presence = createPresenceSlice(client, set, get, refreshQuietly);
    const links = createLinksSlice(client, set, get, refreshQuietly);
    const views = createViewsSlice(client, set, get);
    const peek = createPeekSlice(client, set, get);
    const gallery = createGallerySlice(client);
    const bulk = createBulkSlice(client, get);
    const feed = createFeedSlice(client);
    const drive = createDriveSlice(client, set, get);
    const history = createHistorySlice(client, get);
    const shell = createShellSlice(client, set, get);
    // The tabs keep up with the page tree, the lookups and the spaces, and write `ui.tabs` (ADR-0034).
    api.subscribe(tabs.onStateChange);
    // The peek of a database row closes when the tab, the page or the space in front changes (ADR-0054).
    api.subscribe(peek.onStateChange);

    /** The device-local settings the first paint needs: the layout (ADR-0029), the active views and the peek width (ADR-0054). */
    async function loadLocalSettings(): Promise<Record<string, JsonValue | null>> {
      const [layoutSettings, viewSettings, peekSettings] = await Promise.all([layout.load(), views.load(), peek.load()]);
      return { ...layoutSettings, ...viewSettings, ...peekSettings };
    }

    async function waitForCore(startedAt: number): Promise<CoreWait> {
      for (;;) {
        const state = await client.coreState();
        if (!isCurrent(startedAt)) {
          return { kind: "stopped" };
        }
        if (state === "ready") {
          return { kind: "ready" };
        }
        if (state === "locked") {
          return { kind: "locked" };
        }
        if (state !== "opening") {
          return { kind: "failed", error: state.failed };
        }
        await delay(pollMs);
      }
    }

    async function boot(): Promise<void> {
      const startedAt = epoch;
      set({ phase: "loading", fatalError: null });
      try {
        const core = await waitForCore(startedAt);
        if (core.kind === "stopped") {
          return;
        }
        if (core.kind === "locked") {
          showLocked();
          return;
        }
        if (core.kind === "failed") {
          shell.drop();
          set({ phase: "fatal", fatalError: core.error });
          return;
        }
        // The theme, the accent and the language are read before the app shows, so its first paint already has them
        // (ADR-0040); the language file is loaded here too, so that no frame is drawn in English first.
        const [status, workspace, device, theme, accent, languageSetting] = await Promise.all([
          client.appStatus(),
          client.workspaceGet(),
          client.deviceInfo(),
          client.settingsGet(THEME_KEY),
          client.settingsGet(ACCENT_KEY),
          client.settingsGet(LANGUAGE_KEY),
        ]);
        const language = await loadLanguage(parseLanguageChoice(languageSetting));
        if (!isCurrent(startedAt)) {
          return;
        }
        // Spaces, the security and sync status and the auto-lock delay only exist once there
        // is a workspace; load them before showing the app, so the sidebar does not start empty.
        // The layout settings (sidebar width, recent pages) come with them: the first paint is the final one.
        // So do the stored tabs (ADR-0034): the tree that loads is the one of the active tab's space.
        const storedTabs = workspace === null ? Promise.resolve(null) : tabs.load();
        const [loaded, secured, synced, autoLock, layoutSettings, aiSettings, shared, commented] =
          workspace === null
            ? [null, null, null, undefined, {}, {}, null, null]
            : await Promise.all([
                structure.load(async (spaces) => tabs.activeSpaceFor(await storedTabs, spaces)),
                security.load(),
                sync.load(),
                client.settingsGet(AUTO_LOCK_KEY),
                loadLocalSettings(),
                // Whether AI is on decides what every screen shows (ADR-0037).
                ai.load(),
                // The profile name and an invite session under way (a link that was opened before a reload).
                sharing.load(),
                // The inbox of notifications (the bell's count).
                comments.load(),
              ]);
        // Without one, a pairing or a recovery may be under way: the onboarding resumes it.
        const sessions = workspace === null ? await devices.loadForOnboarding() : null;
        if (!isCurrent(startedAt)) {
          return;
        }
        const stored = await storedTabs;
        const restored = loaded === null ? null : tabs.restore(stored, loaded.spaces);
        const languageChanged = get().language !== language;
        set((state) => ({
          ...loaded,
          ...restored,
          ...secured,
          ...synced,
          ...shared,
          ...commented,
          ...sessions,
          status,
          workspace,
          device,
          settings: {
            ...state.settings,
            ...layoutSettings,
            ...aiSettings,
            [THEME_KEY]: theme,
            [ACCENT_KEY]: accent,
            [LANGUAGE_KEY]: languageSetting,
            ...(autoLock === undefined ? {} : { [AUTO_LOCK_KEY]: autoLock }),
            ...(restored === null ? {} : { [TABS_KEY]: storedTabsValue(stored) }),
          },
          language,
          phase: workspace === null ? "onboarding" : "ready",
          onboardingStep: "workspace",
        }));
        if (languageChanged) {
          shell.sendLabels();
        }
        // A link of the operating system that came while the app was opening or locked opens now (ADR-0040);
        // without a workspace there is nothing to open it in.
        if (workspace === null) {
          shell.drop();
        } else {
          shell.ready();
        }
        // At launch and after an unlock the scheduler looks at what fell due meanwhile at once.
        reminders.tick();
      } catch (error) {
        if (!isCurrent(startedAt)) {
          return;
        }
        const failure = toCoreError(error);
        if (failure.code === "Locked") {
          // Locked between the state check and the requests (by another window).
          showLocked();
          return;
        }
        shell.drop();
        set({ phase: "fatal", fatalError: failure.toApiError() });
      }
    }

    async function refreshStatus(): Promise<void> {
      const status = await client.appStatus();
      set({ status });
    }

    async function refreshWorkspace(): Promise<void> {
      const workspace = await client.workspaceGet();
      set({ workspace });
    }

    async function refreshDevice(): Promise<void> {
      const device = await client.deviceInfo();
      set({ device });
    }

    async function loadSetting(key: string): Promise<void> {
      const value = await client.settingsGet(key);
      set((state) => ({ settings: { ...state.settings, [key]: value } }));
      if (key === LANGUAGE_KEY) {
        // Changed by another window of this device: the words follow.
        await applyLanguage(parseLanguageChoice(value));
      }
    }

    async function refreshSetting(key: string): Promise<void> {
      if (!(key in get().settings)) {
        return; // Nobody has read it, so there is no slice to refresh.
      }
      await loadSetting(key);
    }

    function onEvent(event: CoreEvent): void {
      switch (event.type) {
        case "WorkspaceCreated": {
          const { phase, onboardingStep } = get();
          if (creatingWorkspace === 0 && phase === "onboarding" && onboardingStep === "workspace") {
            // Created elsewhere (another window), or installed by a pairing or a recovery
            // through the kit: start again with it, as at launch.
            void boot();
          } else {
            refreshQuietly(() => Promise.all([refreshWorkspace(), refreshStatus()]));
          }
          break;
        }
        case "SettingChanged":
          refreshQuietly(() => refreshSetting(event.data.key));
          break;
        case "DeviceRenamed":
          refreshQuietly(refreshDevice);
          break;
        // Events added by a newer core are not this UI's business.
      }
      structure.onEvent(event);
      content.onEvent(event);
      security.onEvent(event);
      attachments.onEvent(event);
      sync.onEvent(event);
      devices.onEvent(event);
      reminders.onEvent(event);
      properties.onEvent(event);
      tabs.onEvent(event);
      layout.onEvent(event);
      interop.onEvent(event);
      ai.onEvent(event);
      sharing.onEvent(event);
      comments.onEvent(event);
      presence.onEvent(event);
      links.onEvent(event);
      views.onEvent(event);
      peek.onEvent(event);
      drive.onEvent(event);
    }

    return {
      ...structure.initial,
      ...content.initial,
      ...security.initial,
      ...attachments.initial,
      ...sync.initial,
      ...devices.initial,
      ...reminders.initial,
      ...properties.initial,
      ...layout.initial,
      ...tabs.initial,
      ...interop.initial,
      ...ai.initial,
      ...sharing.initial,
      ...comments.initial,
      ...presence.initial,
      ...links.initial,
      ...views.initial,
      ...peek.initial,
      ...gallery.initial,
      ...bulk.initial,
      ...feed.initial,
      ...history.initial,
      ...drive.initial,
      ...shell.initial,

      phase: "loading",
      fatalError: null,
      onboardingStep: "workspace",
      route: options.initialRoute ?? "home",
      status: null,
      workspace: null,
      device: null,
      settings: {},
      // English (nothing to load) is in use at once; another system language is drawn once its file is in (`start`).
      language: systemLanguage === "en" ? "en" : null,

      start: () => {
        epoch += 1;
        const startedAt = epoch;
        const unsubscribe = client.subscribe(onEvent);
        // What the desktop shell tells the page: links of the operating system, the quick capture, the tray's
        // lock, the state of the capture shortcut (nothing happens without a shell), and the words of the tray.
        const stopShell = client.onShellEvent(shell.receive);
        if (get().language === null) {
          // The system speaks another language: its file loads while the core opens, and the shell gets its words then.
          void applyLanguage("system");
        } else {
          shell.sendLabels();
        }
        // A core with no shell around it is ticked from here (the desktop shell ticks its own).
        const stopScheduler = reminders.startScheduler();
        void boot();
        return () => {
          unsubscribe();
          stopShell();
          stopScheduler();
          if (epoch === startedAt) {
            epoch += 1;
          }
        };
      },

      boot,

      navigate: (route) => {
        set({ route });
      },

      createWorkspace: async (name, firstSpaceName) => {
        const workspace = await ownWorkspace(() => client.workspaceCreate(name, firstSpaceName));
        set({ workspace, onboardingStep: "kit", route: "home" });
        refreshQuietly(refreshStatus);
        // What boot reads for an existing workspace (the spaces come with WorkspaceCreated).
        refreshQuietly(security.refresh);
        refreshQuietly(sync.refresh);
        refreshQuietly(() => loadSetting(AUTO_LOCK_KEY));
      },

      finishOnboarding: () => {
        set({ phase: "ready", onboardingStep: "workspace", route: "home" });
      },

      setSetting: async (key, value) => {
        await client.settingsSet(key, value);
        set((state) => ({ settings: { ...state.settings, [key]: value } }));
        if (key === LANGUAGE_KEY) {
          await applyLanguage(parseLanguageChoice(value));
        }
      },

      loadSetting,

      renameDevice: async (name) => {
        const device = await client.deviceRename(name);
        set({ device });
      },

      refreshStatus,
    };
  });
}
