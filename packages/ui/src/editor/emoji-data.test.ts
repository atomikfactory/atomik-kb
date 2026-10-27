// The emoji index behind the `:` picker and the icon picker's search: how it is built from the
// data file, the aliases, the ranking, and one test that loads the real file.

import { afterEach, describe, expect, it } from "vitest";

import { STAND_IN_EMOJI } from "../test/emoji";
import {
  DEFAULT_EMOJI_CODES,
  EMOJI_ALIASES,
  emojiForCode,
  emojiLoaded,
  loadEmoji,
  searchEmoji,
  setEmojiData,
} from "./emoji-data";
import type { EmojiData, EmojiRecord } from "./emoji-data";

afterEach(() => {
  setEmojiData(STAND_IN_EMOJI); // what test/setup.ts installs
});

function record(slug: string, group = "Objects"): EmojiRecord {
  return { name: slug.replaceAll("_", " "), slug, group };
}

/** The shortcodes `searchEmoji` shows for `query`, best first. */
async function codes(query: string, limit = 50): Promise<string[]> {
  return (await searchEmoji(query, limit)).map((choice) => choice.code);
}

describe("the index", () => {
  it("lists each emoji of the data once, with its slug, name and group", async () => {
    setEmojiData({
      "🐈": { name: "cat", slug: "cat", group: "Animals & Nature" },
      "🔥": { name: "fire", slug: "fire", group: "Travel & Places" },
    });
    expect(await searchEmoji("c", 10)).toEqual([
      { emoji: "🐈", slug: "cat", name: "cat", group: "Animals & Nature", code: "cat" },
    ]);
    expect(await searchEmoji("fire", 10)).toEqual([
      { emoji: "🔥", slug: "fire", name: "fire", group: "Travel & Places", code: "fire" },
    ]);
  });

  it("is built on the stand-in the tests run with, without loading the file", async () => {
    expect(emojiLoaded()).toBe(true);
    expect(await codes("tada")).toEqual(["tada"]);
    expect(emojiForCode("tada")).toBe("🎉");
  });

  it("leaves out skin tone variants: base emoji only", async () => {
    setEmojiData({
      "👋": { name: "waving hand", slug: "waving_hand", group: "People & Body" },
      "👋🏽": { name: "waving hand: medium skin tone", slug: "waving_hand_medium_skin_tone", group: "People & Body" },
      "🏽": { name: "medium skin tone", slug: "medium_skin_tone", group: "Component" },
      // A variant whose slug does not say so is still one: it holds a modifier.
      "👍🏿": { name: "thumbs up dark", slug: "thumbs_up_dark", group: "People & Body" },
      "👍": { name: "thumbs up", slug: "thumbs_up", group: "People & Body" },
    });
    expect(await codes("waving")).toEqual(["waving_hand"]);
    expect(await codes("skin")).toEqual([]);
    expect(await codes("thumbs")).toEqual(["thumbsup"]); // the alias of thumbs_up
    expect(emojiForCode("waving_hand_medium_skin_tone")).toBeNull();
    expect(emojiForCode("medium_skin_tone")).toBeNull();
    expect(emojiForCode("thumbs_up_dark")).toBeNull();
  });

  it("skips records that are not an emoji record", async () => {
    setEmojiData({
      "🐈": record("cat"),
      "?": { name: 3, slug: "broken", group: "x" } as unknown as EmojiRecord,
      "!": null as unknown as EmojiRecord,
    });
    expect(await codes("")).toEqual([]);
    expect(await codes("cat")).toEqual(["cat"]);
    expect(await codes("broken")).toEqual([]);
  });

  it("holds what was set last: a swap discards the data before it", async () => {
    setEmojiData({ "🐈": record("cat") });
    expect(emojiLoaded()).toBe(true);
    setEmojiData({ "🔥": record("fire") });
    expect(emojiForCode("cat")).toBeNull();
    expect(emojiForCode("fire")).toBe("🔥");
    await loadEmoji(); // nothing to load: it is set
    expect(emojiLoaded()).toBe(true);
  });
});

describe("the aliases", () => {
  const DATA: EmojiData = {
    "😄": record("grinning_face_with_smiling_eyes", "Smileys & Emotion"),
    "🎉": record("party_popper", "Activities"),
    "👍": record("thumbs_up", "People & Body"),
    "👎": record("thumbs_down", "People & Body"),
    "❤️": record("red_heart", "Smileys & Emotion"),
  };

  it("name the emoji of their slug, exactly and by prefix, and show as the shortcode", async () => {
    setEmojiData(DATA);
    for (const [alias, emoji] of [
      ["smile", "😄"],
      ["tada", "🎉"],
      ["+1", "👍"],
      ["thumbsup", "👍"],
      ["-1", "👎"],
      ["thumbsdown", "👎"],
      ["heart", "❤️"],
    ] as const) {
      expect(emojiForCode(alias), alias).toBe(emoji);
    }
    expect(emojiForCode("SMILE")).toBe("😄"); // case aside
    expect(await searchEmoji("smi", 5)).toEqual([
      {
        emoji: "😄",
        slug: "grinning_face_with_smiling_eyes",
        name: "grinning face with smiling eyes",
        group: "Smileys & Emotion",
        code: "smile",
      },
    ]);
    expect(await codes("thumbs")).toEqual(["thumbsup", "thumbsdown"]); // the shortest shortcode that matches, in the order of the data
    expect(await codes("party")).toEqual(["party_popper"]); // the slug still finds it
  });

  it("do not stand for an emoji the data lacks", async () => {
    setEmojiData({ "👍": record("thumbs_up") });
    expect(emojiForCode("smile")).toBeNull();
    expect(emojiForCode("tada")).toBeNull();
    expect(await codes("smi")).toEqual([]);
  });

  it("never take the place of a slug", async () => {
    // `heart` is a slug here: the alias of `red_heart` gives way to it.
    setEmojiData({ "❤️": record("red_heart"), "♥️": record("heart") });
    expect(emojiForCode("heart")).toBe("♥️");
    expect(await codes("heart")).toEqual(["heart", "red_heart"]);
  });

  it("are lowercase shortcodes that differ from the slug they stand for", () => {
    for (const [alias, slug] of Object.entries(EMOJI_ALIASES)) {
      expect(alias, alias).toBe(alias.toLowerCase());
      expect(slug, alias).toMatch(/^[a-z0-9_]+$/u);
      expect(alias, alias).not.toBe(slug);
    }
  });
});

describe("ranking", () => {
  const DATA: EmojiData = {
    // Not in the order of the ranking: the order of the data decides only between equals.
    "🐭": { name: "toy mouse", slug: "toy_mouse", group: "Cat Things" }, // the group is not searched
    "🐆": record("bobcat"),
    "🐈‍⬛": record("black_cat"),
    "😼": record("cat_with_wry_smile"),
    "🐱": record("cat_face"),
    "🐈": record("cat"),
    "😺": record("cat_with_tears"),
  };

  it("puts the text itself first, then shortcodes that start with it (the shorter first), names with it as a word, then shortcodes that hold it", async () => {
    setEmojiData(DATA);
    expect(await codes("cat")).toEqual([
      "cat", // exactly it
      "cat_face", // starts with it, the shortest
      "cat_with_tears", // starts with it
      "cat_with_wry_smile",
      "black_cat", // a word of the name
      "bobcat", // inside a shortcode
    ]);
  });

  it("matches words of the name in any order, and the shortcode with spaces for underscores", async () => {
    setEmojiData(STAND_IN_EMOJI);
    expect(await codes("face tears")).toEqual(["face_with_tears_of_joy"]);
    expect(await codes("tears face")).toEqual(["face_with_tears_of_joy"]);
    expect(await codes("grinning face")).toEqual([
      "grinning_face",
      "grinning_face_with_big_eyes",
      "grinning_face_with_smiling_eyes",
    ]);
  });

  it("ranks :smi, :+1 and :tad as people expect", async () => {
    expect(await codes("smi")).toEqual(["smile", "smiley", "smiling_face_with_smiling_eyes", "slightly_smiling_face"]);
    expect(await codes("+1")).toEqual(["+1"]); // not the 1st place medal: the plus is part of the text
    expect(await codes("-1")).toEqual(["-1"]);
    expect(await codes("tad")).toEqual(["tada"]);
    expect(await codes("100")).toEqual(["100"]);
  });

  it("ignores case, accents, colons around the text and spaces around it", async () => {
    setEmojiData({ "🪅": { name: "piñata", slug: "pinata", group: "Activities" } });
    expect(await codes("PIÑ")).toEqual(["pinata"]);
    expect(await codes(":pin:")).toEqual(["pinata"]);
    expect(await codes("  pinata ")).toEqual(["pinata"]);
  });

  it("limits the list, and gives nothing for text with no letter or digit in it", async () => {
    const many: Record<string, EmojiRecord> = {};
    for (let index = 0; index < 30; index += 1) {
      many[`emoji-${String(index)}`] = record(`face_${String(index).padStart(2, "0")}`);
    }
    setEmojiData(many);
    expect(await codes("face", 8)).toHaveLength(8);
    expect(await codes("face", 3)).toEqual(["face_00", "face_01", "face_02"]);
    expect(await codes("face", 0)).toEqual([]);
    expect(await codes("face", -1)).toEqual([]);
    expect(await codes(":-)")).toEqual([]);
    expect(await codes("__")).toEqual([]);
  });

  it("lists the common emoji when nothing is typed", async () => {
    expect(await codes("", 8)).toEqual([...DEFAULT_EMOJI_CODES]);
    expect(await codes("   ", 3)).toEqual(["smile", "+1", "heart"]);
    expect(await codes("::", 2)).toEqual(["smile", "+1"]);
  });
});

describe("the data file", () => {
  it("loads, and finds :tada: and the common shortcodes", async () => {
    setEmojiData(null);
    expect(emojiLoaded()).toBe(false);
    expect(emojiForCode("tada")).toBeNull(); // not loaded yet
    await loadEmoji();
    expect(emojiLoaded()).toBe(true);
    await loadEmoji(); // loaded once

    expect(emojiForCode("tada")).toBe("🎉");
    expect((await searchEmoji("tada", 5))[0]).toMatchObject({ emoji: "🎉", slug: "party_popper", code: "tada" });
    expect(emojiForCode("party_popper")).toBe("🎉");
    expect((await codes("smi"))[0]).toBe("smile");
    expect(await codes("+1", 3)).toEqual(["+1"]);
    expect((await searchEmoji("+1", 1))[0]?.emoji).toBe("👍");
    expect((await searchEmoji("100", 1))[0]?.emoji).toBe("💯");
    expect((await searchEmoji("grinning face", 1))[0]?.emoji).toBe("😀");
  });

  it("holds every alias target and the shortcodes people know from GitHub", async () => {
    setEmojiData(null);
    await loadEmoji();
    for (const [alias, slug] of Object.entries(EMOJI_ALIASES)) {
      const emoji = emojiForCode(slug);
      expect(emoji, `${alias} -> ${slug}`).not.toBeNull();
      expect(emojiForCode(alias), alias).toBe(emoji);
    }
    for (const code of [
      "smile",
      "+1",
      "-1",
      "heart",
      "fire",
      "tada",
      "rocket",
      "eyes",
      "thinking",
      "joy",
      "sob",
      "100",
      "check",
      "white_check_mark",
      "x",
      "warning",
      "bulb",
      "memo",
      "star",
      "zap",
      "sparkles",
      "wave",
      "pray",
      "clap",
      "ok_hand",
      "point_right",
      "question",
      "exclamation",
      "calendar",
      "bug",
      "lock",
    ]) {
      expect(emojiForCode(code), code).not.toBeNull();
    }
    expect((await searchEmoji("", 20)).map((choice) => choice.code)).toEqual([...DEFAULT_EMOJI_CODES]);
  });

  it("holds no skin tone variant and no component", async () => {
    setEmojiData(null);
    await loadEmoji();
    expect(await codes("skin_tone")).toEqual([]);
    expect(emojiForCode("light_skin_tone")).toBeNull();
    expect(emojiForCode("waving_hand")).toBe("👋");
    const all = await searchEmoji("e", 5000); // the file has about 1,900 emoji; most have an `e` somewhere
    expect(all.length).toBeGreaterThan(1000);
    expect(all.some((choice) => /[\u{1F3FB}-\u{1F3FF}]/u.test(choice.emoji))).toBe(false);
  });
});
