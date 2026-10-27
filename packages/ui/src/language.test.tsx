// The language of the UI (ADR-0040 item 13) in the whole app over the in-memory core: the device setting `ui.language`
// is read at boot with the theme and the accent and its file is loaded before the app shows, so no frame is in English;
// `system` follows `navigator.languages`; Settings has the picker; `<html lang>` and the formats follow the language;
// and the shell gets its words (tray, menu bar, dialogs) in it, at start and on every change.

import { CoreClient, CoreError, MemoryTransport } from "@atomik-kb/core-client";
import type { ShellLabels } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";

import { formatBytes, formatPageCount, formatTimestamp } from "./format";
import { activeLanguage, activeLocale, resetLanguage, t } from "./i18n";
import messages from "./i18n/en.json";
import { formatNumber } from "./property-format";
import { createAppStore } from "./store";
import type { AppState } from "./store";
import { NOW, findMainNavigation, openScreen, renderApp, renderWith } from "./test/render";
import { withShell } from "./test/shell";

const FRENCH = JSON.parse(readFileSync("src/i18n/fr.json", "utf8")) as Record<string, string>;
const SPANISH = JSON.parse(readFileSync("src/i18n/es.json", "utf8")) as Record<string, string>;
const ENGLISH = messages as Record<string, string>;

/** Makes `navigator.languages` (and `navigator.language`) say this; `afterEach` puts the real ones back. */
function systemSays(languages: readonly string[]): void {
  Object.defineProperty(navigator, "languages", { configurable: true, value: languages });
  Object.defineProperty(navigator, "language", { configurable: true, value: languages[0] ?? "" });
}

afterEach(() => {
  resetLanguage();
  Reflect.deleteProperty(navigator, "languages");
  Reflect.deleteProperty(navigator, "language");
  document.documentElement.removeAttribute("lang");
  vi.restoreAllMocks();
});

const navigation = (language: Record<string, string>) => screen.queryByRole("navigation", { name: language["nav.label"] });

describe("the language at boot", () => {
  it("is read with the theme and the accent: the first frame of the app is already in it", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", settings: { "ui.language": "fr" } });
    const store = createAppStore(new CoreClient(memory));
    const frames: Pick<AppState, "phase" | "language">[] = [];
    store.subscribe((state) => {
      frames.push({ phase: state.phase, language: state.language });
    });
    const stop = store.getState().start();
    await waitFor(() => {
      expect(store.getState().phase).toBe("ready");
    });
    stop();

    // No state that shows the app (every phase but loading) is in a language other than the stored one,
    // and the language is in use by the time the store says so.
    const shown = frames.filter((frame) => frame.phase !== "loading");
    expect(shown.length).toBeGreaterThan(0);
    expect(shown.every((frame) => frame.language === "fr")).toBe(true);
    expect(activeLanguage()).toBe("fr");
    expect(t("nav.settings")).toBe(FRENCH["nav.settings"]);
  });

  it("draws the app in the stored language and puts it on <html lang>", async () => {
    renderApp({ workspaceName: "Research", settings: { "ui.language": "fr" } });
    expect(await screen.findByRole("navigation", { name: FRENCH["nav.label"] })).toBeInTheDocument();
    expect(navigation(ENGLISH)).toBeNull();
    expect(document.documentElement).toHaveAttribute("lang", "fr");
  });

  it("is English, and `lang` en, when the setting is absent and the system speaks English", async () => {
    renderApp({ workspaceName: "Research" });
    await findMainNavigation();
    expect(navigation(ENGLISH)).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute("lang", "en");
  });

  it("follows the system for `system` and for no setting at all: its first language that the app has", async () => {
    systemSays(["fr-CA", "en-US"]);
    renderApp({ workspaceName: "Research", settings: { "ui.language": "system" } });
    expect(await screen.findByRole("navigation", { name: FRENCH["nav.label"] })).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute("lang", "fr");
    expect(activeLocale()).toBe("fr-CA");
  });

  it("draws nothing while the system's language file loads, then the first frame is in it", async () => {
    systemSays(["fr-FR"]);
    const { container } = renderApp({ workspaceName: "Research" });
    // The first render: the store has no language yet, and no screen (not even the loading one) is in English.
    expect(container).toBeEmptyDOMElement();
    expect(await screen.findByRole("navigation", { name: FRENCH["nav.label"] })).toBeInTheDocument();
    expect(navigation(ENGLISH)).toBeNull();
  });

  it("follows a Spanish system: es-ES draws the app in Spanish, with the region the system names", async () => {
    systemSays(["es-ES", "en-US"]);
    renderApp({ workspaceName: "Research" });
    expect(await screen.findByRole("navigation", { name: SPANISH["nav.label"] })).toBeInTheDocument();
    expect(navigation(ENGLISH)).toBeNull();
    expect(document.documentElement).toHaveAttribute("lang", "es");
    expect(activeLanguage()).toBe("es");
    expect(activeLocale()).toBe("es-ES");
  });

  it("takes any Spanish region for Spanish: es-MX", async () => {
    systemSays(["es-MX"]);
    renderApp({ workspaceName: "Research" });
    expect(await screen.findByRole("navigation", { name: SPANISH["nav.label"] })).toBeInTheDocument();
    expect(activeLocale()).toBe("es-MX");
  });

  it("draws the stored language in Spanish whatever the system says", async () => {
    systemSays(["fr-FR"]);
    renderApp({ workspaceName: "Research", settings: { "ui.language": "es" } });
    expect(await screen.findByRole("navigation", { name: SPANISH["nav.label"] })).toBeInTheDocument();
    expect(navigation(FRENCH)).toBeNull();
    expect(document.documentElement).toHaveAttribute("lang", "es");
  });

  it("is English for a system language the app does not have", async () => {
    systemSays(["de-DE", "ja"]);
    renderApp({ workspaceName: "Research" });
    await findMainNavigation();
    expect(navigation(ENGLISH)).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute("lang", "en");
  });

  it("takes a chosen language over the system's: English on a French system", async () => {
    systemSays(["fr-FR"]);
    renderApp({ workspaceName: "Research", settings: { "ui.language": "en" } });
    expect(await screen.findByRole("navigation", { name: ENGLISH["nav.label"] })).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute("lang", "en");
  });

  it("takes a stored value it does not understand (a language of a newer version) for `system`", async () => {
    systemSays(["fr-FR"]);
    renderApp({ workspaceName: "Research", settings: { "ui.language": "tlh" } });
    expect(await screen.findByRole("navigation", { name: FRENCH["nav.label"] })).toBeInTheDocument();
  });

  it("keeps the language on the lock screen once the app was opened in it", async () => {
    renderApp({ workspaceName: "Research", settings: { "ui.language": "fr" } });
    await screen.findByRole("navigation", { name: FRENCH["nav.label"] });
    expect(activeLanguage()).toBe("fr");
  });
});

const languageGroup = () => screen.getByRole("group", { name: t("settings.language.legend") });
const languageRadio = (name: string) => within(languageGroup()).getByRole("radio", { name });

/** Opens Settings in an app that starts in `language` (the helpers of `test/render` name things in the language active when they are called, before boot has loaded another). */
async function openSettings(options: Parameters<typeof renderApp>[0] = {}, language: Record<string, string> = ENGLISH) {
  const app = renderApp({ workspaceName: "Research", ...options });
  const nav = await screen.findByRole("navigation", { name: language["nav.label"] });
  await app.user.click(within(nav).getByRole("button", { name: language["nav.settings"] }));
  return app;
}

describe("settings: language", () => {
  it("offers Match system and each language by its own name, with Match system checked by default", async () => {
    await openSettings();
    const radios = within(languageGroup()).getAllByRole("radio");
    expect(radios.map((radio) => radio.closest("label")?.textContent)).toEqual([t("settings.language.system"), "English", "Français", "Español"]);
    expect(languageRadio(t("settings.language.system"))).toBeChecked();
    // Each name is marked as the language it is written in.
    expect(within(languageGroup()).getByText("Français")).toHaveAttribute("lang", "fr");
  });

  it("does not write the setting just by opening Settings", async () => {
    const { memory } = await openSettings();
    await expect(memory.dispatch({ type: "SettingsGet", data: { key: "ui.language" } })).resolves.toEqual({ type: "Setting", data: null });
  });

  it("shows the stored choice", async () => {
    await openSettings({ settings: { "ui.language": "fr" } }, FRENCH);
    expect(languageRadio("Français")).toBeChecked();
  });

  it("draws the whole app again in the language that is chosen, and keeps the choice", async () => {
    const { user, memory } = await openSettings();
    await user.click(languageRadio("Français"));

    // The legend, the sidebar and the title are French now, and the page says so.
    expect(await screen.findByRole("heading", { level: 1, name: FRENCH["settings.title"] })).toBeInTheDocument();
    expect(screen.getByRole("navigation", { name: FRENCH["nav.label"] })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 1, name: ENGLISH["settings.title"] })).toBeNull();
    expect(document.documentElement).toHaveAttribute("lang", "fr");
    expect(activeLanguage()).toBe("fr");
    await expect(memory.dispatch({ type: "SettingsGet", data: { key: "ui.language" } })).resolves.toEqual({ type: "Setting", data: "fr" });

    // The picker, still on Settings, is French too, and checked.
    expect(screen.getByRole("group", { name: FRENCH["settings.language.legend"] })).toBeInTheDocument();
    expect(languageRadio("Français")).toBeChecked();

    // And back to English.
    await user.click(languageRadio("English"));
    expect(await screen.findByRole("heading", { level: 1, name: ENGLISH["settings.title"] })).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute("lang", "en");
  });

  it("chooses Match system back: the system's language follows", async () => {
    systemSays(["fr-FR"]);
    const { user, memory } = await openSettings({ settings: { "ui.language": "en" } });
    expect(await screen.findByRole("heading", { level: 1, name: ENGLISH["settings.title"] })).toBeInTheDocument();
    await user.click(languageRadio(ENGLISH["settings.language.system"] ?? ""));
    expect(await screen.findByRole("heading", { level: 1, name: FRENCH["settings.title"] })).toBeInTheDocument();
    await expect(memory.dispatch({ type: "SettingsGet", data: { key: "ui.language" } })).resolves.toEqual({ type: "Setting", data: "system" });
  });

  it("keeps the language, and says so, when saving the choice fails", async () => {
    const { user, memory } = await openSettings();
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    await user.click(languageRadio("Français"));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("settings.language.error", { message: "database error: disk I/O error" }));
    expect(activeLanguage()).toBe("en");
    expect(languageRadio(t("settings.language.system"))).toBeChecked();
    expect(document.documentElement).toHaveAttribute("lang", "en");
  });

  it("follows a language written by another window of this device, through the SettingChanged event", async () => {
    const { memory } = await openSettings();
    await memory.dispatch({ type: "SettingsSet", data: { key: "ui.language", value: "fr" } });
    expect(await screen.findByRole("heading", { level: 1, name: FRENCH["settings.title"] })).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute("lang", "fr");
  });

  it("lists Spanish, and choosing it draws the whole app again in Spanish and keeps the choice", async () => {
    const { user, memory } = await openSettings();
    await user.click(languageRadio("Español"));
    expect(await screen.findByRole("heading", { level: 1, name: SPANISH["settings.title"] })).toBeInTheDocument();
    expect(screen.getByRole("navigation", { name: SPANISH["nav.label"] })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 1, name: ENGLISH["settings.title"] })).toBeNull();
    expect(document.documentElement).toHaveAttribute("lang", "es");
    expect(activeLanguage()).toBe("es");
    await expect(memory.dispatch({ type: "SettingsGet", data: { key: "ui.language" } })).resolves.toEqual({ type: "Setting", data: "es" });
    // The picker, still on Settings, is Spanish too, and checked; each language keeps its own name.
    expect(screen.getByRole("group", { name: SPANISH["settings.language.legend"] })).toBeInTheDocument();
    expect(languageRadio("Español")).toBeChecked();
    expect(within(languageGroup()).getByText("Español")).toHaveAttribute("lang", "es");
    expect(within(languageGroup()).getByText("English")).toBeInTheDocument();
  });
});

describe("the formats", () => {
  it("follow the language: numbers, sizes, dates and the plural of a count", async () => {
    systemSays(["fr-FR"]);
    renderApp({ workspaceName: "Research" });
    await screen.findByRole("navigation", { name: FRENCH["nav.label"] });

    expect(activeLocale()).toBe("fr-FR");
    expect(formatNumber(1234.5).replace(/\s/g, " ")).toBe("1 234,5");
    expect(formatBytes(1536).replace(/\s/g, " ")).toBe("1,5 ko");
    expect(formatPageCount(0)).toBe(`0 page`);
    expect(formatPageCount(3)).toBe("3 pages");
    const when = formatTimestamp(Date.UTC(2026, 9, 14, 12, 30));
    expect(when?.text).toMatch(/14 oct\. 2026/);
  });

  it("follow Spanish too: the number, the plural of a count and the date", async () => {
    systemSays(["es-ES"]);
    renderApp({ workspaceName: "Research" });
    await screen.findByRole("navigation", { name: SPANISH["nav.label"] });

    expect(activeLocale()).toBe("es-ES");
    expect(formatNumber(1234.5).replace(/\s/g, " ")).toBe("1234,5");
    expect(formatPageCount(0)).toBe("0 páginas");
    expect(formatPageCount(1)).toBe("1 página");
    expect(formatPageCount(3)).toBe("3 páginas");
    expect(formatTimestamp(Date.UTC(2026, 9, 14, 12, 30))?.text).toMatch(/^14 oct 2026/);
  });

  it("stay English on an English system, with the region the system names", async () => {
    systemSays(["en-GB"]);
    renderApp({ workspaceName: "Research" });
    await findMainNavigation();
    expect(activeLocale()).toBe("en-GB");
    expect(formatPageCount(1)).toBe("1 page");
    expect(formatTimestamp(Date.UTC(2026, 9, 14, 12, 30))?.text).toMatch(/^14 Oct 2026/);
  });
});

/** Every `{placeholder}` of a text. */
const placeholdersOf = (text: string): string[] => [...text.matchAll(/\{(\w+)\}/g)].map((match) => match[1] ?? "");

/** What the shell requires of each dialog text (`shell_labels.rs`): the placeholders a consent depends on. */
const REQUIRED: Readonly<Record<string, readonly string[]>> = {
  dialogAiMessage: ["host"],
  dialogMcpRead: ["client", "spaces"],
  dialogMcpWrite: ["client", "spaces"],
  dialogMcpBoth: ["client", "read", "write"],
  dialogListAnd: ["names", "last"],
  dialogListMore: ["count"],
  dialogImagesOne: ["stored"],
  dialogImagesOther: ["count", "stored"],
  dialogHostsOne: ["host"],
  dialogHostsOther: ["host", "count"],
  // The body of the OS notifications of comments and mentions names the person (M13 slice 2).
  dialogNotifyThreadReply: ["actor"],
  dialogNotifyCommentMention: ["actor"],
  dialogNotifyPageMention: ["actor"],
  dialogNotifyPersonAssigned: ["actor"],
  // The download of a speech model for dictation says how much and from where (ADR-0053); `{model}` may be used too.
  dialogDictationMessage: ["size", "host"],
  // The sign-in to Google Drive (ADR-0059) and the page the browser lands on after it: texts of their own words, no placeholder.
  dialogDriveMessage: [],
  dialogDriveDone: [],
};

function setupShell(options: Parameters<typeof renderApp>[0] = {}) {
  const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", ...options });
  const shell = withShell(memory);
  const app = renderWith(shell.transport);
  return { ...app, memory, shell };
}

/** The shell's own validation of a message of labels, in TypeScript (the Rust tests have the real one). */
function checkLabels(labels: ShellLabels): void {
  for (const [id, text] of Object.entries(labels) as [string, string][]) {
    // eslint-disable-next-line no-control-regex -- the shell refuses control characters
    expect(/[\u0000-\u001f\u007f]/.test(text), id).toBe(false);
    expect(text.trim(), id).toBe(text);
    const dialogText = Object.hasOwn(REQUIRED, id);
    expect(text.length, id).toBeGreaterThanOrEqual(1);
    expect(text.length, id).toBeLessThanOrEqual(dialogText ? 400 : 64);
    const needed = REQUIRED[id];
    if (needed !== undefined) {
      expect(placeholdersOf(text).sort(), id).toEqual(expect.arrayContaining([...needed]));
      // Only the placeholders the shell fills for this text.
      for (const name of placeholdersOf(text)) {
        expect(["host", "client", "spaces", "read", "write", "names", "last", "count", "stored", "actor", "page", "model", "size"], `${id}: {${name}}`).toContain(name);
      }
    }
  }
}

describe("the words the shell gets", () => {
  it("are English at start, with the dialogs' texts and the file types", async () => {
    const { shell } = setupShell();
    await findMainNavigation();
    expect(shell.labels).toHaveLength(1);
    const labels = shell.labels[0] ?? {};
    expect(labels).toMatchObject({
      open: ENGLISH["tray.open"],
      dialogAllow: "Allow",
      dialogYes: "Yes",
      dialogAiTitle: "Send text to another computer?",
      dialogMcpRead: "Allow {client} to read {spaces}?",
      fileFilterZip: "Zip archive",
    });
    // Every dialog and file label of the UI is sent, from its `shellDialog.*` or `fileFilter.*` string.
    const sent = Object.entries(labels).filter(([id]) => id.startsWith("dialog") || id.startsWith("fileFilter"));
    expect(sent).toHaveLength(34); // the five of the Google Drive sign-in (ADR-0059) are among them
    for (const [id, text] of sent) {
      const key = id.startsWith("dialog") ? `shellDialog.${id.charAt(6).toLowerCase()}${id.slice(7)}` : `fileFilter.${id.charAt(10).toLowerCase()}${id.slice(11)}`;
      expect(ENGLISH[key], id).toBe(text);
    }
    checkLabels(labels);
  });

  it("are the stored language's once it is read, and valid for the shell", async () => {
    const { shell } = setupShell({ settings: { "ui.language": "fr" } });
    await screen.findByRole("navigation", { name: FRENCH["nav.label"] });
    await waitFor(() => {
      expect(shell.labels.length).toBeGreaterThanOrEqual(1);
    });
    const last = shell.labels.at(-1) ?? {};
    expect(last).toMatchObject({
      open: FRENCH["tray.open"],
      newPage: FRENCH["tray.newPage"],
      menuFile: FRENCH["menu.file"],
      dialogAiTitle: FRENCH["shellDialog.aiTitle"],
      dialogAiMessage: FRENCH["shellDialog.aiMessage"],
      fileFilterZip: FRENCH["fileFilter.zip"],
    });
    checkLabels(last);
    // The consent still says where the text goes.
    expect(last.dialogAiMessage).toContain("{host}");
  });

  it("are Spanish for a Spanish system, valid for the shell, and the consents keep what they ask about", async () => {
    systemSays(["es-ES"]);
    const { shell } = setupShell();
    await screen.findByRole("navigation", { name: SPANISH["nav.label"] });
    await waitFor(() => {
      expect(shell.labels.length).toBeGreaterThanOrEqual(1);
    });
    const last = shell.labels.at(-1) ?? {};
    expect(last).toMatchObject({
      open: SPANISH["tray.open"],
      quit: SPANISH["tray.quit"],
      menuFile: SPANISH["menu.file"],
      dialogAllow: SPANISH["shellDialog.allow"],
      dialogAiTitle: SPANISH["shellDialog.aiTitle"],
      fileFilterBackup: SPANISH["fileFilter.backup"],
    });
    checkLabels(last);
    // Each consent still says where the text goes, who asks and what for.
    expect(last.dialogAiMessage).toContain("{host}");
    expect(last.dialogMcpRead).toMatch(/\{client\}.*\{spaces\}/u);
    expect(last.dialogMcpWrite).toMatch(/\{client\}.*\{spaces\}/u);
    expect(last.dialogMcpBoth).toMatch(/\{client\}.*\{read\}.*\{write\}/u);
    expect(last.dialogImagesOne).toContain("{stored}");
    expect(last.dialogImagesOther).toContain("{stored}");
  });

  it("are sent again, translated, when the language is changed in Settings, and again when it is changed back", async () => {
    const { user, shell } = setupShell();
    await openScreen(user, "nav.settings");
    expect(shell.labels).toHaveLength(1);

    await user.click(languageRadio("Français"));
    await waitFor(() => {
      expect(shell.labels).toHaveLength(2);
    });
    expect(shell.labels[1]).toMatchObject({ quit: FRENCH["tray.quit"], menuEdit: FRENCH["menu.edit"], dialogCancel: FRENCH["shellDialog.cancel"] });
    checkLabels(shell.labels[1] ?? {});

    // And back: the last message is the English one, word for word what the app started with.
    await screen.findByRole("heading", { level: 1, name: FRENCH["settings.title"] });
    await user.click(languageRadio("English"));
    await waitFor(() => {
      expect(shell.labels.at(-1)).toEqual(shell.labels[0]);
    });
  });

  it("are not sent again when the setting is written with the language it already has", async () => {
    const { user, shell, memory } = setupShell({ settings: { "ui.language": "en" } });
    await openScreen(user, "nav.settings");
    const before = shell.labels.length;
    await memory.dispatch({ type: "SettingsSet", data: { key: "ui.language", value: "system" } });
    await act(async () => {
      await Promise.resolve();
    });
    expect(shell.labels).toHaveLength(before);
  });

  it("do not keep the language from changing when the shell refuses them", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research" });
    const shell = withShell(memory);
    const { user } = renderWith(shell.transport);
    await openScreen(user, "nav.settings");
    shell.failLabels(new CoreError({ code: "InvalidInput", message: "a label is 1 to 64 characters: SECRETLABEL" }));
    await user.click(languageRadio("Français"));
    expect(await screen.findByRole("heading", { level: 1, name: FRENCH["settings.title"] })).toBeInTheDocument();
    await waitFor(() => {
      expect(logged).toHaveBeenCalledWith("the shell's labels could not be set", "InvalidInput");
    });
    expect(JSON.stringify(logged.mock.calls)).not.toContain("SECRETLABEL");
  });
});
