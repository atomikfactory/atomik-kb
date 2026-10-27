// Shared helpers for the recovery kit, app lock and backup tests.

import { screen, within } from "@testing-library/react";

import { t } from "../i18n";
import type { User } from "./render";

/** The app-lock passphrase the tests (and the dev page) use. */
export const PASSPHRASE = "correct horse battery";

/** The words of the kit on screen, in order, read from its numbered grid. */
export function shownWords(): string[] {
  const list = screen.getByRole("list", { name: t("kit.wordsLabel") });
  return within(list)
    .getAllByRole("listitem")
    .map((item) => item.querySelector(".kit-word-text")?.textContent ?? "");
}

/** The numbers shown next to the words, in order. */
export function shownNumbers(): string[] {
  const list = screen.getByRole("list", { name: t("kit.wordsLabel") });
  return within(list)
    .getAllByRole("listitem")
    .map((item) => item.querySelector(".kit-word-number")?.textContent ?? "");
}

/** The kit's code on screen. */
export function shownCode(): string {
  return screen.getByRole("group", { name: t("kit.codeLabel") }).textContent ?? "";
}

/** The 1-based positions the check asks for, read from the labels of its fields. */
export function askedPositions(): number[] {
  const label = new RegExp(`^${t("kit.confirm.word", { position: "(\\d+)" })}$`);
  return screen.getAllByRole("textbox").flatMap((box) => {
    const match = label.exec((box as HTMLInputElement).labels?.[0]?.textContent ?? "");
    return match === null ? [] : [Number(match[1])];
  });
}

/** Types the asked words, taken from `words` (through `transform`), and presses "Check". */
export async function answerCheck(
  user: User,
  words: readonly string[],
  transform: (word: string) => string = (word) => word,
): Promise<void> {
  for (const position of askedPositions()) {
    await user.type(
      screen.getByRole("textbox", { name: t("kit.confirm.word", { position }) }),
      transform(words[position - 1] ?? ""),
    );
  }
  await user.click(screen.getByRole("button", { name: t("kit.confirm.submit") }));
}

/**
 * The whole kit flow from its explanation: create, read the words, check three of them,
 * "Done". Returns the words that were shown.
 */
export async function completeKitFlow(user: User): Promise<string[]> {
  await user.click(await screen.findByRole("button", { name: t("kit.create") }));
  await screen.findByRole("list", { name: t("kit.wordsLabel") });
  const words = shownWords();
  await user.click(screen.getByRole("button", { name: t("kit.saved") }));
  await answerCheck(user, words);
  await user.click(await screen.findByRole("button", { name: t("kit.done") }));
  return words;
}
