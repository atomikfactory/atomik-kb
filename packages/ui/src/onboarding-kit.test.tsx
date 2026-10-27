import { BIP39_ENGLISH, CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import { NOW, findMainNavigation, openScreen, recordingTransport, renderApp, renderWith } from "./test/render";
import type { User } from "./test/render";
import { answerCheck, askedPositions, shownCode, shownNumbers, shownWords } from "./test/security";

afterEach(() => {
  vi.restoreAllMocks();
  Reflect.deleteProperty(navigator, "clipboard");
});

/** Names the workspace and waits for the recovery kit step. */
async function toKitStep(user: User, name = "Personal"): Promise<void> {
  await user.type(await screen.findByLabelText(t("onboarding.nameLabel")), `${name}{Enter}`);
  await screen.findByRole("heading", { name: t("kit.title"), level: 1 });
}

/** Creates the kit from its explanation and waits for the words. */
async function createKit(user: User): Promise<string[]> {
  await user.click(screen.getByRole("button", { name: t("kit.create") }));
  await screen.findByRole("list", { name: t("kit.wordsLabel") });
  return shownWords();
}

async function kitOf(memory: MemoryTransport) {
  return (await new CoreClient(memory).securityStatus()).recoveryKit;
}

describe("onboarding: the recovery kit step", () => {
  it("explains the kit and warns before creating it", async () => {
    const { user, memory } = renderApp();
    await toKitStep(user);
    expect(screen.getByText(t("kit.intro"))).toBeInTheDocument();
    expect(screen.getByText(t("kit.warning"))).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("kit.create") })).toBeEnabled();
    expect(screen.getByRole("button", { name: t("kit.later") })).toBeEnabled();
    expect(screen.queryByRole("list", { name: t("kit.wordsLabel") })).not.toBeInTheDocument();
    // Nothing is created before the user asks for it.
    expect(await kitOf(memory)).toBeUndefined();
  });

  it("shows the 24 words of the new kit in a numbered grid", async () => {
    const { user, memory } = renderApp();
    await toKitStep(user);
    const words = await createKit(user);
    expect(words).toHaveLength(24);
    expect(words.every((word) => BIP39_ENGLISH.includes(word))).toBe(true);
    expect(shownNumbers()).toEqual(Array.from({ length: 24 }, (_, index) => String(index + 1)));
    expect(screen.getByText(t("kit.show.lead"))).toBeInTheDocument();
    expect(screen.getByText(t("kit.warning"))).toBeInTheDocument();
    expect(await kitOf(memory)).toEqual({ createdAt: NOW, stale: false });
    // The words are the kit the core now checks against.
    await expect(new CoreClient(memory).recoveryKitVerify(words.join(" "))).resolves.toBeUndefined();
  });

  it("switches between the words and the code", async () => {
    const { user } = renderApp();
    await toKitStep(user);
    await createKit(user);
    const switcher = screen.getByRole("group", { name: t("kit.formLabel") });
    expect(within(switcher).getByRole("button", { name: t("kit.form.words") })).toHaveAttribute("aria-pressed", "true");

    await user.click(within(switcher).getByRole("button", { name: t("kit.form.code") }));
    expect(within(switcher).getByRole("button", { name: t("kit.form.code") })).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByRole("list", { name: t("kit.wordsLabel") })).not.toBeInTheDocument();
    expect(shownCode()).toMatch(/^[A-Z2-7]{4}(-[A-Z2-7]{4}){12}-[A-Z2-7]{2}$/);

    await user.click(within(switcher).getByRole("button", { name: t("kit.form.words") }));
    expect(screen.getByRole("list", { name: t("kit.wordsLabel") })).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: t("kit.codeLabel") })).not.toBeInTheDocument();
  });

  it("copies the form on screen to the clipboard", async () => {
    // user-event puts a clipboard of its own on `navigator` for every test.
    const { user } = renderApp();
    await toKitStep(user);
    const words = await createKit(user);
    await user.click(screen.getByRole("button", { name: t("kit.copy") }));
    expect(await screen.findByText(t("kit.copied"))).toBeInTheDocument();
    expect(await navigator.clipboard.readText()).toBe(words.join(" "));

    await user.click(screen.getByRole("button", { name: t("kit.form.code") }));
    const code = shownCode();
    await user.click(screen.getByRole("button", { name: t("kit.copy") }));
    await waitFor(async () => {
      expect(await navigator.clipboard.readText()).toBe(code);
    });
  });

  it("shows the kit to copy by hand where the clipboard cannot be written", async () => {
    const { user } = renderApp();
    await toKitStep(user);
    const words = await createKit(user);
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(new Error("not allowed"));
    await user.click(screen.getByRole("button", { name: t("kit.copy") }));

    const dialog = await screen.findByRole("dialog", { name: t("kit.copyDialog.title") });
    expect(within(dialog).getByRole("textbox")).toHaveValue(words.join(" "));
    await user.click(within(dialog).getByRole("button", { name: t("dialog.close") }));
    expect(screen.queryByRole("dialog", { name: t("kit.copyDialog.title") })).not.toBeInTheDocument();
    expect(screen.getByRole("list", { name: t("kit.wordsLabel") })).toBeInTheDocument();
  });

  it("prints a sheet that holds only the kit", async () => {
    const print = vi.spyOn(window, "print").mockImplementation(() => undefined);
    const { user } = renderApp();
    await toKitStep(user);
    const words = await createKit(user);
    await user.click(screen.getByRole("button", { name: t("kit.form.code") }));
    const code = shownCode();
    await user.click(screen.getByRole("button", { name: t("kit.print") }));
    expect(print).toHaveBeenCalledTimes(1);

    // A direct child of the body (the print styles hide every other one), hidden from assistive technology.
    const sheet = document.querySelector("body > .kit-print-sheet");
    expect(sheet).not.toBeNull();
    expect(sheet).toHaveAttribute("aria-hidden", "true");
    const printed = [...(sheet?.querySelectorAll(".kit-word-text") ?? [])].map((word) => word.textContent);
    expect(printed).toEqual(words);
    expect(sheet).toHaveTextContent(code);
    expect(sheet).toHaveTextContent(t("kit.print.warning"));

    // Gone with the kit's display.
    await user.click(screen.getByRole("button", { name: t("kit.saved") }));
    expect(document.querySelector(".kit-print-sheet")).toBeNull();
  });

  it("checks three words, then verifies the whole kit with the core and finishes", async () => {
    const memory = new MemoryTransport({ now: () => NOW });
    const recorder = recordingTransport(memory);
    const { user } = renderWith(recorder.transport);
    await toKitStep(user);
    const words = await createKit(user);
    await user.click(screen.getByRole("button", { name: t("kit.saved") }));

    const asked = askedPositions();
    expect(asked).toHaveLength(3);
    expect(new Set(asked).size).toBe(3);
    expect(asked.every((position) => position >= 1 && position <= 24)).toBe(true);
    expect(screen.getByRole("textbox", { name: t("kit.confirm.word", { position: asked[0] ?? 0 }) })).toHaveFocus();

    await answerCheck(user, words);
    expect(await screen.findByText(t("kit.done.lead"))).toBeInTheDocument();
    expect(recorder.requests.filter((request) => request.type === "RecoveryKitVerify")).toEqual([
      { type: "RecoveryKitVerify", data: { kit: words.join(" ") } },
    ]);
    expect(await kitOf(memory)).toEqual({ createdAt: NOW, verifiedAt: NOW, stale: false });
    // Checked: nothing on screen (or on the print sheet) holds the kit any more.
    expect(document.querySelector(".kit-word-text")).toBeNull();
    expect(document.querySelector(".kit-code")).toBeNull();

    await user.click(screen.getByRole("button", { name: t("kit.done") }));
    await findMainNavigation();
    expect(screen.getByRole("heading", { name: "Personal", level: 1 })).toBeInTheDocument();
    expect(screen.queryByRole("complementary", { name: t("kitBanner.label") })).not.toBeInTheDocument();
  });

  it("accepts the first four letters of a word, in any case", async () => {
    const { user } = renderApp();
    await toKitStep(user);
    const words = await createKit(user);
    await user.click(screen.getByRole("button", { name: t("kit.saved") }));
    await answerCheck(user, words, (word) => (word.length > 4 ? word.slice(0, 4) : word).toUpperCase());
    expect(await screen.findByText(t("kit.done.lead"))).toBeInTheDocument();
  });

  it("refuses wrong words without asking the core, and asks the same words after another look at the kit", async () => {
    const memory = new MemoryTransport({ now: () => NOW });
    const recorder = recordingTransport(memory);
    const { user } = renderWith(recorder.transport);
    await toKitStep(user);
    const words = await createKit(user);
    await user.click(screen.getByRole("button", { name: t("kit.saved") }));
    const asked = askedPositions();
    const [first = 1, second = 2] = asked;

    // One wrong word among right ones.
    await answerCheck(user, words, (word) => (word === words[second - 1] ? "zzzz" : word));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("kit.confirm.error.mismatch"));
    expect(screen.getByRole("textbox", { name: t("kit.confirm.word", { position: second }) })).toHaveAttribute(
      "aria-invalid",
      "true",
    );
    expect(screen.getByRole("textbox", { name: t("kit.confirm.word", { position: first }) })).toHaveAttribute(
      "aria-invalid",
      "false",
    );
    expect(recorder.requests.filter((request) => request.type === "RecoveryKitVerify")).toEqual([]);

    await user.click(screen.getByRole("button", { name: t("kit.confirm.back") }));
    expect(shownWords()).toEqual(words);
    await user.click(screen.getByRole("button", { name: t("kit.saved") }));
    expect(askedPositions()).toEqual(asked);
    await answerCheck(user, words);
    expect(await screen.findByText(t("kit.done.lead"))).toBeInTheDocument();
  });

  it("asks for all three words before checking", async () => {
    const { user } = renderApp();
    await toKitStep(user);
    await createKit(user);
    await user.click(screen.getByRole("button", { name: t("kit.saved") }));
    await user.click(screen.getByRole("button", { name: t("kit.confirm.submit") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("kit.confirm.error.empty"));
  });

  it("says when the core does not verify the kit", async () => {
    const { user, memory } = renderApp();
    await toKitStep(user);
    const words = await createKit(user);
    await user.click(screen.getByRole("button", { name: t("kit.saved") }));
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    await answerCheck(user, words);
    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("kit.confirm.error.generic", { message: "database error: disk I/O error" }),
    );
    expect(screen.getByRole("button", { name: t("kit.confirm.submit") })).toBeEnabled();
  });

  it("reports a kit that could not be created and tries again", async () => {
    const { user, memory } = renderApp();
    await toKitStep(user);
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    await user.click(screen.getByRole("button", { name: t("kit.create") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("kit.error.create", { message: "database error: disk I/O error" }),
    );
    await user.click(screen.getByRole("button", { name: t("kit.retry") }));
    expect(await screen.findByRole("list", { name: t("kit.wordsLabel") })).toBeInTheDocument();
  });

  it("'Do this later' goes to the app without a kit and leaves a reminder on Home and in Settings", async () => {
    const { user, memory } = renderApp();
    await toKitStep(user);
    await user.click(screen.getByRole("button", { name: t("kit.later") }));
    await findMainNavigation();
    expect(await kitOf(memory)).toBeUndefined();

    const banner = await screen.findByRole("complementary", { name: t("kitBanner.label") });
    expect(banner).toHaveTextContent(t("kitBanner.body"));
    await openScreen(user, "nav.settings");
    expect(await screen.findByRole("complementary", { name: t("kitBanner.label") })).toBeInTheDocument();
  });

  it("runs the kit flow from the reminder, which goes away once the kit exists", async () => {
    const { user, memory } = renderApp();
    await toKitStep(user);
    await user.click(screen.getByRole("button", { name: t("kit.later") }));
    const banner = await screen.findByRole("complementary", { name: t("kitBanner.label") });
    await user.click(within(banner).getByRole("button", { name: t("kitBanner.action") }));

    const dialog = await screen.findByRole("dialog", { name: t("kit.title") });
    await user.click(within(dialog).getByRole("button", { name: t("kit.create") }));
    await within(dialog).findByRole("list", { name: t("kit.wordsLabel") });
    // The kit exists now: the reminder goes, the dialog showing the kit stays, even on Escape.
    await waitFor(() => {
      expect(screen.queryByRole("complementary", { name: t("kitBanner.label") })).not.toBeInTheDocument();
    });
    await user.keyboard("{Escape}");
    expect(screen.getByRole("dialog", { name: t("kit.title") })).toBeInTheDocument();

    const words = shownWords();
    await user.click(within(dialog).getByRole("button", { name: t("kit.saved") }));
    await answerCheck(user, words);
    await user.click(await within(dialog).findByRole("button", { name: t("kit.done") }));
    expect(screen.queryByRole("dialog", { name: t("kit.title") })).not.toBeInTheDocument();
    expect(await kitOf(memory)).toEqual({ createdAt: NOW, verifiedAt: NOW, stale: false });
  });

  it("closes the reminder's dialog before a kit exists with 'Do this later' or Escape", async () => {
    const { user } = renderApp({ workspaceName: "Research" });
    const banner = await screen.findByRole("complementary", { name: t("kitBanner.label") });
    await user.click(within(banner).getByRole("button", { name: t("kitBanner.action") }));
    const dialog = await screen.findByRole("dialog", { name: t("kit.title") });
    await user.click(within(dialog).getByRole("button", { name: t("kit.later") }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    await user.click(within(banner).getByRole("button", { name: t("kitBanner.action") }));
    await screen.findByRole("dialog", { name: t("kit.title") });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("kitBanner.action") })).toHaveFocus();
  });
});
