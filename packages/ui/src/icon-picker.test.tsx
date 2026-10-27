// The icon picker (pages, spaces, callouts): free typing and the suggestions as before, and a
// search over the bundled emoji set above the suggestions.

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { IconPicker } from "./components/IconPicker";
import { setEmojiData } from "./editor/emoji-data";
import { t } from "./i18n";
import { STAND_IN_EMOJI } from "./test/emoji";

function setup(current?: string) {
  const onSelect = vi.fn<(icon: string | null) => Promise<void>>(() => Promise.resolve());
  const onClose = vi.fn();
  const user = userEvent.setup();
  render(<IconPicker anchor={{ x: 10, y: 10 }} current={current} onSelect={onSelect} onClose={onClose} />);
  const picker = screen.getByRole("dialog", { name: t("icon.title") });
  return { user, onSelect, onClose, picker };
}

function searchField(): HTMLElement {
  return screen.getByRole("searchbox", { name: t("emoji.search") });
}

/** The emoji of the buttons of the group `name`. */
function glyphs(picker: HTMLElement, name: string): (string | null)[] {
  return within(within(picker).getByRole("group", { name })).getAllByRole("button").map((button) => button.textContent);
}

describe("the icon picker", () => {
  it("keeps free typing first, and the 16 suggestions", async () => {
    const { user, onSelect, picker } = setup();
    expect(within(picker).getByRole("textbox", { name: t("icon.label") })).toHaveFocus();
    expect(glyphs(picker, t("icon.suggestions"))).toHaveLength(16);
    expect(within(picker).queryByRole("group", { name: t("emoji.results") })).not.toBeInTheDocument();

    await user.type(within(picker).getByRole("textbox", { name: t("icon.label") }), "🚀{Enter}");
    expect(onSelect).toHaveBeenCalledWith("🚀");
  });

  it("offers a search field under the typed icon, above the suggestions", () => {
    const { picker } = setup();
    const field = searchField();
    expect(field).toHaveAttribute("placeholder", t("emoji.search.placeholder"));
    const suggestions = within(picker).getByRole("group", { name: t("icon.suggestions") });
    expect(field.compareDocumentPosition(suggestions) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("searches the emoji by name and shows what it finds in place of the suggestions", async () => {
    const { user, onSelect, picker } = setup();
    await user.type(searchField(), "party");
    const results = await within(picker).findByRole("group", { name: t("emoji.results") });
    expect(within(results).getAllByRole("button")).toHaveLength(1);
    const button = within(results).getByRole("button", { name: "party popper" });
    expect(button).toHaveTextContent("🎉");
    expect(within(picker).queryByRole("group", { name: t("icon.suggestions") })).not.toBeInTheDocument();

    await user.click(button);
    expect(onSelect).toHaveBeenCalledWith("🎉");
  });

  it("finds by shortcode too: tada, +1, smile, a word of the name", async () => {
    const { user, picker } = setup();
    for (const [typed, expected] of [
      [":tada", ["🎉"]],
      ["+1", ["👍"]],
      ["smile", ["😄", "😃"]], // `smile` and `smiley`
      ["tears", ["😂"]],
    ] as const) {
      await user.clear(searchField());
      await user.type(searchField(), typed);
      await waitFor(() => {
        expect(glyphs(picker, t("emoji.results")), typed).toEqual(expected);
      });
    }
  });

  it("lists at most 24, and takes the first one with Enter", async () => {
    const { user, onSelect, picker } = setup();
    setEmojiData({
      ...STAND_IN_EMOJI,
      ...Object.fromEntries(
        Array.from({ length: 40 }, (_unused, index) => [
          `face-${String(index)}`,
          { name: `face ${String(index)}`, slug: `face_${String(index).padStart(2, "0")}`, group: "Smileys & Emotion" },
        ]),
      ),
    });
    try {
      await user.type(searchField(), "face");
      await waitFor(() => {
        expect(glyphs(picker, t("emoji.results"))).toHaveLength(24);
      });
      await user.keyboard("{Enter}");
      expect(onSelect).toHaveBeenCalledTimes(1);
      expect(onSelect).toHaveBeenCalledWith("face-0"); // `face_00`: the shortest slug that starts with it
    } finally {
      setEmojiData(STAND_IN_EMOJI);
    }
  });

  it("says so when nothing matches, does nothing on Enter, and brings the suggestions back when cleared", async () => {
    const { user, onSelect, picker } = setup();
    await user.type(searchField(), "zzzq");
    expect(await within(picker).findByText(t("emoji.noMatch"))).toBeInTheDocument();
    expect(within(picker).queryByRole("group")).not.toBeInTheDocument();
    await user.keyboard("{Enter}");
    expect(onSelect).not.toHaveBeenCalled();

    await user.clear(searchField());
    expect(within(picker).queryByText(t("emoji.noMatch"))).not.toBeInTheDocument();
    expect(glyphs(picker, t("icon.suggestions"))).toHaveLength(16);
  });

  it("shows nothing stale while the answer to a new text is on its way", async () => {
    const { user, picker } = setup();
    await user.type(searchField(), "party");
    await within(picker).findByRole("group", { name: t("emoji.results") });
    await user.clear(searchField());
    expect(within(picker).queryByRole("group", { name: t("emoji.results") })).not.toBeInTheDocument();
    expect(within(picker).getByRole("group", { name: t("icon.suggestions") })).toBeInTheDocument();
  });

  it("removes an icon as before", async () => {
    const { user, onSelect, picker } = setup("🎯");
    await user.click(within(picker).getByRole("button", { name: t("icon.remove") }));
    expect(onSelect).toHaveBeenCalledWith(null);
  });
});
