// The shared-space screens in French and Spanish (Milestone 13): every `sharing.*` text of the dialogs, the member panel, the banners and
// the invitations is in the language chosen on this device, with its placeholders filled (a text never shows `{name}`), and the
// roles are named as the glossary names them.

import { MemoryTransport } from "@atomik-kb/core-client";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";

import { resetLanguage } from "./i18n";
import { NOW, renderWith } from "./test/render";
import { SERVER, SHARING_SPACES } from "./test/sharing";

type Dictionary = Record<string, string>;

const LANGUAGES: readonly [string, Dictionary][] = [
  ["fr", JSON.parse(readFileSync("src/i18n/fr.json", "utf8")) as Dictionary],
  ["es", JSON.parse(readFileSync("src/i18n/es.json", "utf8")) as Dictionary],
];

afterEach(() => {
  resetLanguage();
  document.documentElement.removeAttribute("lang");
  vi.restoreAllMocks();
});

/** A text of a dictionary with its placeholders filled. */
function text(dictionary: Dictionary, key: string, params: Record<string, string | number> = {}): string {
  const raw = dictionary[key];
  if (raw === undefined) {
    throw new Error(`no ${key}`);
  }
  return raw.replace(/\{(\w+)\}/gu, (_match, name: string) => String(params[name] ?? `{${name}}`));
}

/**
 * The same, as a text on the page is matched: the page's text has its white space collapsed, and French writes a no-break space
 * before a colon, a semicolon and inside the guillemets, which the matcher would not equate with a plain space.
 */
function plain(dictionary: Dictionary, key: string, params: Record<string, string | number> = {}): string {
  return text(dictionary, key, params).replace(/\s+/gu, " ");
}

async function render(language: string, dictionary: Dictionary, shared: boolean) {
  const memory = new MemoryTransport({
    now: () => NOW,
    workspaceName: "Research",
    spaces: SHARING_SPACES,
    security: { kitCreatedAt: NOW },
    sync: { state: "idle", serverUrl: SERVER },
    syncStepMs: 5,
    settings: { "ui.language": language },
    sharing: { profileName: "Maya", stepMs: 30, ...(shared ? { spaces: [{ space: "Work" }] } : {}) },
  });
  const app = renderWith(memory);
  await screen.findByRole("navigation", { name: text(dictionary, "nav.label") });
  // Work is the second space.
  await app.user.click(screen.getByRole("button", { description: text(dictionary, "spaces.switchTitle") }));
  await app.user.click(await screen.findByRole("button", { name: /^Work/u }));
  await waitFor(() => {
    expect(screen.getByRole("button", { description: text(dictionary, "spaces.switchTitle") })).toHaveTextContent("Work");
  });
  return app;
}

async function choose(user: ReturnType<typeof userEvent.setup>, dictionary: Dictionary, key: string): Promise<void> {
  await user.click(screen.getByRole("button", { name: text(dictionary, "spaces.optionsLabel") }));
  const menu = await screen.findByRole("menu", { name: text(dictionary, "spaces.optionsLabel") });
  await user.click(within(menu).getByRole("menuitem", { name: text(dictionary, key) }));
}

describe.each(LANGUAGES)("in %s", (language, dictionary) => {
  it("shows the share dialog in the language, with the roles, the history warning and the AI note", async () => {
    const { user } = await render(language, dictionary, false);
    await choose(user, dictionary, "sharing.share.menu");
    const dialog = await screen.findByRole("dialog", { name: text(dictionary, "sharing.share.title", { name: "Work" }) });
    for (const key of [
      "sharing.share.intro",
      "sharing.share.history",
      "sharing.share.ai",
      "sharing.role.owner",
      "sharing.role.editor",
      "sharing.role.commenter",
      "sharing.role.viewer",
      "sharing.role.owner.hint",
      "sharing.role.viewer.hint",
    ]) {
      expect(within(dialog).getByText(plain(dictionary, key)), key).toBeInTheDocument();
    }
    expect(within(dialog).getByRole("button", { name: text(dictionary, "sharing.share.submit") })).toBeInTheDocument();
    expect(dialog.textContent).not.toMatch(/\{\w+\}/u);
  });

  it("shows the member panel, the invitations and the settings in the language", async () => {
    const { user } = await render(language, dictionary, true);
    await choose(user, dictionary, "sharing.menu.members");
    const dialog = await screen.findByRole("dialog", { name: text(dictionary, "sharing.members.title") });
    await within(dialog).findByRole("list", { name: text(dictionary, "sharing.members.list") });
    for (const key of [
      "sharing.members.heading",
      "sharing.invites.title",
      "sharing.settings.title",
      "sharing.ai.title",
      "sharing.invites.none",
      "sharing.settings.retentionHint",
    ]) {
      expect(dialog.textContent.replace(/\s+/gu, " "), key).toContain(plain(dictionary, key));
    }
    expect(within(dialog).getByRole("button", { name: text(dictionary, "sharing.invites.code") })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: text(dictionary, "sharing.invites.link") })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: text(dictionary, "sharing.leave.button") })).toBeInTheDocument();
    expect(within(dialog).getByText(text(dictionary, "sharing.members.verified"))).toBeInTheDocument();
    expect(within(dialog).getAllByText(text(dictionary, "sharing.members.unverified")).length).toBeGreaterThan(0);
    expect(dialog.textContent).not.toMatch(/\{\w+\}/u);
    // The question before leaving says what happens, in the language.
    await userEvent.setup().click(within(dialog).getByRole("button", { name: text(dictionary, "sharing.close.button") }));
    const question = await screen.findByRole("alertdialog", { name: text(dictionary, "sharing.close.title", { name: "Work" }) });
    expect(question).toHaveTextContent(plain(dictionary, "sharing.close.body"));
  });

  it("says in the language that a space is a read-only copy", async () => {
    const memory = new MemoryTransport({
      now: () => NOW,
      workspaceName: "Research",
      spaces: SHARING_SPACES,
      security: { kitCreatedAt: NOW },
      sync: { state: "idle", serverUrl: SERVER },
      settings: { "ui.language": language },
      sharing: { profileName: "Maya", spaces: [{ space: "Work", role: "editor", archive: "Removed" }] },
    });
    const app = renderWith(memory);
    await screen.findByRole("navigation", { name: text(dictionary, "nav.label") });
    await app.user.click(screen.getByRole("button", { description: text(dictionary, "spaces.switchTitle") }));
    await app.user.click(await screen.findByRole("button", { name: /^Work/u }));
    expect(await screen.findByText(plain(dictionary, "sharing.archive.removed"))).toBeInTheDocument();
    expect(screen.getByRole("button", { name: text(dictionary, "sharing.archive.remove") })).toBeInTheDocument();
  });
});
