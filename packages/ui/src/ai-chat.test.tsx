// The chat panel (ADR-0037, components/AiChatPanel.tsx and MainLayout): the toggle and Ctrl+J exist only with AI on,
// a question streams in an answer with sources and citation chips, Stop, scope, a new chat, the actions on an
// answer (copy, save as a page, insert into the open page), and the panel with AI off or without a chat model.

import { CoreClient } from "@atomik-kb/core-client";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import { AI_ON, AI_SETTING, AI_SPACES, aiOf } from "./test/ai";
import { contentOf, idsOf, openPage, renderPages } from "./test/page";
import type { User } from "./test/render";

afterEach(() => {
  vi.restoreAllMocks();
});

async function render(options: Parameters<typeof renderPages>[1] = {}) {
  return renderPages(AI_SPACES, { ai: { mode: "fake" }, settings: { [AI_SETTING]: AI_ON }, ...options });
}

const toggle = () => screen.getByRole("button", { name: t("aiChat.toggle") });
const panel = () => screen.getByRole("complementary", { name: t("aiChat.title") });
const box = () => within(panel()).getByRole("textbox", { name: t("aiChat.placeholder") });
const log = () => within(panel()).getByRole("log", { name: t("aiChat.log") });

async function ask(user: User, question: string): Promise<void> {
  await user.type(box(), `${question}{Enter}`);
}

/** Opens the panel with the button. */
async function openPanel(user: User): Promise<void> {
  await user.click(toggle());
  await screen.findByRole("complementary", { name: t("aiChat.title") });
}

function answerOf(index = 0): HTMLElement {
  const answers = log().querySelectorAll<HTMLElement>(".ai-msg-assistant");
  const answer = answers[index];
  if (answer === undefined) {
    throw new Error("no such answer");
  }
  return answer;
}

describe("the entry points", () => {
  it("do not exist where the transport has no AI", async () => {
    await renderPages(AI_SPACES);
    expect(screen.queryByRole("button", { name: t("aiChat.toggle") })).not.toBeInTheDocument();
    fireEvent.keyDown(document.body, { key: "j", ctrlKey: true });
    expect(screen.queryByRole("complementary", { name: t("aiChat.title") })).not.toBeInTheDocument();
  });

  it("do not exist while AI is off", async () => {
    await renderPages(AI_SPACES, { ai: { mode: "fake" } });
    expect(screen.queryByRole("button", { name: t("aiChat.toggle") })).not.toBeInTheDocument();
    fireEvent.keyDown(document.body, { key: "j", ctrlKey: true });
    expect(screen.queryByRole("complementary", { name: t("aiChat.title") })).not.toBeInTheDocument();
  });

  it("are a star at the end of the tab strip and Ctrl+J, which toggle the panel and focus the box", async () => {
    const { user } = await render();
    expect(toggle()).toHaveAttribute("aria-pressed", "false");
    expect(toggle()).toHaveAttribute("title", t("aiChat.toggleTitle", { shortcut: "Ctrl+J" }));
    expect(screen.queryByRole("complementary", { name: t("aiChat.title") })).not.toBeInTheDocument();
    await user.click(toggle());
    expect(toggle()).toHaveAttribute("aria-pressed", "true");
    expect(panel()).toBeInTheDocument();
    expect(box()).toHaveFocus();
    await user.click(toggle());
    expect(screen.queryByRole("complementary", { name: t("aiChat.title") })).not.toBeInTheDocument();
    fireEvent.keyDown(document.body, { key: "j", ctrlKey: true });
    expect(await screen.findByRole("complementary", { name: t("aiChat.title") })).toBeInTheDocument();
    fireEvent.keyDown(document.body, { key: "j", ctrlKey: true });
    await waitFor(() => {
      expect(screen.queryByRole("complementary", { name: t("aiChat.title") })).not.toBeInTheDocument();
    });
    // Cmd+J too, and neither with Shift or Alt.
    fireEvent.keyDown(document.body, { key: "j", metaKey: true });
    expect(await screen.findByRole("complementary", { name: t("aiChat.title") })).toBeInTheDocument();
    fireEvent.keyDown(document.body, { key: "J", metaKey: true, shiftKey: true });
    fireEvent.keyDown(document.body, { key: "j", ctrlKey: true, altKey: true });
    expect(panel()).toBeInTheDocument();
  });

  it("sits beside the screen under the tab strip, all three children of the main area", async () => {
    const { user } = await render();
    await openPanel(user);
    const main = panel().parentElement;
    expect(main).toHaveClass("content", "has-ai-toggle", "has-chat");
    expect(main?.querySelector(":scope > .tab-strip")).not.toBeNull();
    expect(main?.querySelector(":scope > .content-body")).not.toBeNull();
    await user.click(toggle());
    expect(toggle().closest(".content")).not.toHaveClass("has-chat");
  });

  it("close with the panel's own button", async () => {
    const { user } = await render();
    await openPanel(user);
    await user.click(within(panel()).getByRole("button", { name: t("aiChat.close") }));
    expect(screen.queryByRole("complementary", { name: t("aiChat.title") })).not.toBeInTheDocument();
    expect(toggle()).toHaveAttribute("aria-pressed", "false");
  });

  it("do not fire Ctrl+J from a dialog", async () => {
    const { user } = await render();
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    const dialog = await screen.findByRole("dialog", { name: t("search.label") });
    await user.type(within(dialog).getByRole("combobox"), "x");
    fireEvent.keyDown(within(dialog).getByRole("combobox"), { key: "j", ctrlKey: true });
    expect(screen.queryByRole("complementary", { name: t("aiChat.title") })).not.toBeInTheDocument();
  });
});

describe("asking", () => {
  it("shows the question, streams the answer in, and ends with its citations as chips and its sources listed", async () => {
    const { user, memory } = await render();
    await openPanel(user);
    expect(within(panel()).getByText(t("aiChat.empty"))).toBeInTheDocument();
    const release = aiOf(memory).holdStreams(1);
    await ask(user, "When do I water the tomatoes?");
    expect(box()).toHaveValue("");
    expect(within(log()).getByText("When do I water the tomatoes?")).toBeInTheDocument();
    // While it streams: Stop instead of Send, and no actions yet.
    expect(await within(panel()).findByRole("button", { name: t("aiChat.stop") })).toBeInTheDocument();
    expect(within(panel()).queryByRole("button", { name: t("aiChat.send") })).not.toBeInTheDocument();
    expect(within(answerOf()).queryByRole("button", { name: t("aiChat.copy") })).not.toBeInTheDocument();
    release();
    const chip = await within(answerOf()).findByRole("button", { name: t("aiChat.cite", { n: 1, title: "Garden plans" }) });
    expect(chip).toBeInTheDocument();
    expect(within(panel()).getByRole("button", { name: t("aiChat.send") })).toBeInTheDocument();
    // The sources are listed, collapsed.
    const details = answerOf().querySelector("details");
    expect(details).not.toHaveAttribute("open");
    expect(within(answerOf()).getByText(/^\d+ sources$/)).toBeInTheDocument();
    expect(within(details as HTMLElement).getAllByRole("button", { name: /Garden plans/ }).length).toBeGreaterThan(0);
    expect(within(answerOf()).getByRole("button", { name: t("aiChat.copy") })).toBeInTheDocument();
  });

  it("sends with Enter and not with Shift+Enter, and never an empty question", async () => {
    const { user, memory } = await render();
    await openPanel(user);
    expect(within(panel()).getByRole("button", { name: t("aiChat.send") })).toBeDisabled();
    await user.type(box(), "first line{Shift>}{Enter}{/Shift}second line");
    expect(box()).toHaveValue("first line\nsecond line");
    expect(aiOf(memory).calls.filter((call) => call.kind === "chat")).toHaveLength(0);
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(aiOf(memory).calls.filter((call) => call.kind === "chat")).toHaveLength(1);
    });
    await user.type(box(), "   {Enter}");
    expect(aiOf(memory).calls.filter((call) => call.kind === "chat")).toHaveLength(1);
  });

  it("sends with the Send button too", async () => {
    const { user } = await render();
    await openPanel(user);
    await user.type(box(), "tomatoes");
    await user.click(within(panel()).getByRole("button", { name: t("aiChat.send") }));
    expect(await within(answerOf()).findByRole("button", { name: t("aiChat.copy") })).toBeInTheDocument();
  });

  it("stops an answer: Stop cancels it, and the text so far stays with a note", async () => {
    const { user, memory } = await render();
    await openPanel(user);
    const release = aiOf(memory).holdStreams(1);
    await ask(user, "When do I water the tomatoes?");
    await user.click(await within(panel()).findByRole("button", { name: t("aiChat.stop") }));
    expect(await within(answerOf()).findByText(t("aiChat.stopped"))).toBeInTheDocument();
    release();
    expect(answerOf().textContent).not.toBe(t("aiChat.stopped"));
    expect(within(panel()).getByRole("button", { name: t("aiChat.send") })).toBeInTheDocument();
    // A stopped answer can still be copied.
    expect(within(answerOf()).getByRole("button", { name: t("aiChat.copy") })).toBeInTheDocument();
  });

  it("says why an answer failed", async () => {
    const { user, memory } = await render();
    await openPanel(user);
    aiOf(memory).failNext({ code: "InvalidInput", message: "no", details: { field: "endpoint", reason: "unreachable" } });
    await ask(user, "hello there");
    expect(await within(answerOf()).findByRole("alert")).toHaveTextContent(t("ai.error.unreachable"));
    expect(within(answerOf()).queryByRole("button", { name: t("aiChat.copy") })).not.toBeInTheDocument();
  });

  it("keeps the conversation going, and a new chat forgets it", async () => {
    const { user } = await render();
    await openPanel(user);
    expect(within(panel()).getByRole("button", { name: t("aiChat.new") })).toBeDisabled();
    await ask(user, "tomatoes");
    await within(answerOf(0)).findByRole("button", { name: t("aiChat.copy") });
    await ask(user, "soup");
    await waitFor(() => {
      expect(log().querySelectorAll(".ai-msg-assistant")).toHaveLength(2);
    });
    await within(answerOf(1)).findByRole("button", { name: t("aiChat.copy") });
    await user.click(within(panel()).getByRole("button", { name: t("aiChat.new") }));
    expect(log().querySelectorAll(".ai-msg")).toHaveLength(0);
    expect(within(panel()).getByText(t("aiChat.empty"))).toBeInTheDocument();
  });

  it("keeps the conversation when the panel is closed and opened again", async () => {
    const { user } = await render();
    await openPanel(user);
    await ask(user, "tomatoes");
    await within(answerOf()).findByRole("button", { name: t("aiChat.copy") });
    await user.click(toggle());
    await user.click(toggle());
    expect(within(log()).getByText("tomatoes")).toBeInTheDocument();
  });
});

describe("the scope", () => {
  it("is this space by default, and offers all spaces; this page only with a page open", async () => {
    const { user, memory } = await render();
    await openPanel(user);
    const scope = within(panel()).getByRole("combobox", { name: t("aiChat.scope") });
    expect(scope).toHaveValue("space");
    expect(within(scope).getAllByRole("option").map((option) => option.textContent)).toEqual([t("aiChat.scope.space"), t("aiChat.scope.all")]);
    await openPage(user, "Garden plans");
    await waitFor(() => {
      expect(within(scope).getAllByRole("option")).toHaveLength(3);
    });
    expect(within(scope).getByRole("option", { name: t("aiChat.scope.page") })).toBeInTheDocument();
    await user.selectOptions(scope, t("aiChat.scope.page"));
    await ask(user, "tomato");
    const ids = await idsOf(memory);
    await waitFor(() => {
      expect(aiOf(memory).calls.filter((call) => call.kind === "chat").at(-1)).toMatchObject({
        input: { scope: { type: "page", pageId: ids.page("Garden plans") } },
      });
    });
  });

  it("searches this space unless told otherwise: the roadmap of another space is found only with all spaces", async () => {
    const { user } = await render();
    await openPanel(user);
    await ask(user, "tracker roadmap");
    await within(answerOf(0)).findByText(/could not find/);
    await user.selectOptions(within(panel()).getByRole("combobox", { name: t("aiChat.scope") }), t("aiChat.scope.all"));
    await ask(user, "tracker roadmap");
    expect(await within(answerOf(1)).findByRole("button", { name: t("aiChat.cite", { n: 1, title: "Roadmap" }) })).toBeInTheDocument();
  });

  it("falls back to this space when the page closes", async () => {
    const { user } = await render();
    await openPanel(user);
    await openPage(user, "Garden plans");
    const scope = within(panel()).getByRole("combobox", { name: t("aiChat.scope") });
    await waitFor(() => {
      expect(within(scope).getAllByRole("option")).toHaveLength(3);
    });
    await user.selectOptions(scope, t("aiChat.scope.page"));
    expect(scope).toHaveValue("page");
    await user.click(screen.getByRole("tab", { name: /Garden plans/ }));
    fireEvent.keyDown(document.body, { key: "w", ctrlKey: true });
    await waitFor(() => {
      expect(within(panel()).getByRole("combobox", { name: t("aiChat.scope") })).toHaveValue("space");
    });
  });
});

describe("citations", () => {
  it("open the page and block of the source, also in another space", async () => {
    const scrolled = vi.spyOn(Element.prototype, "scrollIntoView");
    const { user } = await render();
    await openPanel(user);
    await user.selectOptions(within(panel()).getByRole("combobox", { name: t("aiChat.scope") }), t("aiChat.scope.all"));
    await ask(user, "tracker roadmap");
    await user.click(await within(answerOf()).findByRole("button", { name: t("aiChat.cite", { n: 1, title: "Roadmap" }) }));
    expect(await screen.findByRole("heading", { level: 1, name: "Roadmap" })).toBeInTheDocument();
    await waitFor(() => {
      expect(scrolled).toHaveBeenCalled();
    });
  });

  it("open from the sources list too", async () => {
    const { user } = await render();
    await openPanel(user);
    await ask(user, "tomatoes");
    const answer = await within(log()).findByText(/^\d+ sources$/);
    await user.click(answer);
    await user.click(within(answerOf()).getAllByRole("button", { name: /Cooking/ })[0] as HTMLElement);
    expect(await screen.findByRole("heading", { level: 1, name: "Cooking" })).toBeInTheDocument();
  });
});

describe("the actions on an answer", () => {
  it("copies it", async () => {
    const { user } = await render();
    await openPanel(user);
    await ask(user, "tomatoes");
    await user.click(await within(answerOf()).findByRole("button", { name: t("aiChat.copy") }));
    const copied = await navigator.clipboard.readText();
    expect(copied).toContain("According to your notes");
    expect(copied).toContain("[1]");
    expect(await screen.findByText(t("aiChat.copied"))).toBeInTheDocument();
  });

  it("says when it cannot copy", async () => {
    const { user } = await render();
    await openPanel(user);
    await ask(user, "tomatoes");
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(new Error("denied"));
    await user.click(await within(answerOf()).findByRole("button", { name: t("aiChat.copy") }));
    expect(await screen.findByText(t("aiChat.copyFailed"))).toBeInTheDocument();
  });

  it("inserts it at the end of the open page, in one go", async () => {
    const { user, memory } = await render();
    await openPanel(user);
    const editor = await openPage(user, "Garden plans");
    await ask(user, "tomatoes");
    const insert = await within(answerOf()).findByRole("button", { name: t("aiChat.insert") });
    expect(insert).toBeEnabled();
    const before = editor.state.doc.childCount;
    await user.click(insert);
    expect(await screen.findByText(t("aiChat.inserted"))).toBeInTheDocument();
    expect(editor.state.doc.childCount).toBeGreaterThan(before);
    expect(editor.state.doc.lastChild?.textContent).toContain("According to your notes");
    const ids = await idsOf(memory);
    await waitFor(async () => {
      expect(JSON.stringify(await contentOf(memory, ids.page("Garden plans")))).toContain("According to your notes");
    });
  });

  it("cannot insert without an open page", async () => {
    const { user } = await render();
    await openPanel(user);
    await ask(user, "tomatoes");
    expect(await within(answerOf()).findByRole("button", { name: t("aiChat.insert") })).toBeDisabled();
  });

  it("saves it as a page of the current space, titled with the question, with the answer and its sources", async () => {
    const { user, memory } = await render();
    await openPanel(user);
    await ask(user, "When do I water the tomatoes?");
    await user.click(await within(answerOf()).findByRole("button", { name: t("aiChat.save") }));
    expect(await screen.findByRole("heading", { level: 1, name: "When do I water the tomatoes?" })).toBeInTheDocument();
    const client = new CoreClient(memory);
    const [personal] = await client.spaceList();
    const tree = await client.treeGet(personal?.id ?? "");
    const saved = tree.roots.find((node) => node.page.title === "When do I water the tomatoes?");
    expect(saved).toBeDefined();
    await waitFor(async () => {
      const markdown = await client.pageExportMarkdown(saved?.page.id ?? "");
      expect(markdown).toContain("According to your notes");
      expect(markdown).toContain(t("aiChat.sources"));
      expect(markdown).toContain("Garden plans");
    });
    expect(await screen.findByText(t("aiChat.saved", { title: "When do I water the tomatoes?" }))).toBeInTheDocument();
    // The panel keeps the conversation.
    expect(log().querySelectorAll(".ai-msg-assistant")).toHaveLength(1);
  });
});

describe("with AI off or without a chat model", () => {
  it("explains and points to Settings when AI is turned off while the panel is open", async () => {
    const { user, memory } = await render();
    await openPanel(user);
    await new CoreClient(memory).settingsSet(AI_SETTING, { enabled: false });
    expect(await within(panel()).findByText(t("aiChat.off"))).toBeInTheDocument();
    expect(within(panel()).queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: t("aiChat.toggle") })).not.toBeInTheDocument();
    await user.click(within(panel()).getByRole("button", { name: t("aiChat.openSettings") }));
    expect(await screen.findByRole("heading", { name: t("settings.title") })).toBeInTheDocument();
  });

  it("asks for a chat model: the box stays off", async () => {
    const { user } = await render({ settings: { [AI_SETTING]: { enabled: true } } });
    await openPanel(user);
    expect(within(panel()).getByText(t("aiChat.noModel"))).toBeInTheDocument();
    expect(box()).toBeDisabled();
    expect(within(panel()).getByRole("button", { name: t("aiChat.send") })).toBeDisabled();
  });
});

describe("locking", () => {
  it("forgets the conversation and closes the panel", async () => {
    const { user } = await render({ security: { kitCreatedAt: 1, appLockPassphrase: "correct horse battery" } });
    await openPanel(user);
    await ask(user, "tomatoes");
    await within(answerOf()).findByRole("button", { name: t("aiChat.copy") });
    await user.click(within(screen.getByRole("navigation", { name: t("nav.label") })).getByRole("button", { name: t("sidebar.lockNow") }));
    await screen.findByLabelText(t("lock.passphraseLabel"));
    expect(screen.queryByText("tomatoes")).not.toBeInTheDocument();
    await user.type(screen.getByLabelText(t("lock.passphraseLabel")), "correct horse battery");
    await user.click(screen.getByRole("button", { name: t("lock.submit") }));
    await screen.findByRole("navigation", { name: t("nav.label") });
    expect(screen.queryByRole("complementary", { name: t("aiChat.title") })).not.toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: t("aiChat.toggle") }));
    expect(log().querySelectorAll(".ai-msg")).toHaveLength(0);
  });
});
