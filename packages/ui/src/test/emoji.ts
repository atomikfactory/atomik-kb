// A small stand-in for the emoji data file (`unicode-emoji-json/data-by-emoji.json`, about
// 390 kB): `test/setup.ts` installs it, so that tests of the `:` picker and the icon picker
// never load the real file (emoji-data.test.ts has the one test that does). It holds the emoji
// the tests look for, with the slugs and names of the real file.

import type { EmojiData, EmojiRecord } from "../editor/emoji-data";

const FACES = "Smileys & Emotion";
const PEOPLE = "People & Body";

function record(slug: string, group = FACES): EmojiRecord {
  return { name: slug.replaceAll("_", " "), slug, group };
}

export const STAND_IN_EMOJI: EmojiData = {
  "😀": record("grinning_face"),
  "😃": record("grinning_face_with_big_eyes"),
  "😄": record("grinning_face_with_smiling_eyes"),
  "😊": record("smiling_face_with_smiling_eyes"),
  "🙂": record("slightly_smiling_face"),
  "😂": record("face_with_tears_of_joy"),
  "😭": record("loudly_crying_face"),
  "🤔": record("thinking_face"),
  "❤️": record("red_heart"),
  "👍": record("thumbs_up", PEOPLE),
  "👎": record("thumbs_down", PEOPLE),
  "👋": record("waving_hand", PEOPLE),
  "🙏": record("folded_hands", PEOPLE),
  "👏": record("clapping_hands", PEOPLE),
  "👌": record("ok_hand", PEOPLE),
  "👉": record("backhand_index_pointing_right", PEOPLE),
  "👀": record("eyes", PEOPLE),
  "🐛": record("bug", "Animals & Nature"),
  "🔥": record("fire", "Travel & Places"),
  "✨": record("sparkles", "Travel & Places"),
  "🎉": record("party_popper", "Activities"),
  "🚀": record("rocket", "Travel & Places"),
  "⭐": record("star", "Travel & Places"),
  "⚡": record("high_voltage", "Travel & Places"),
  "💡": record("light_bulb", "Objects"),
  "📝": record("memo", "Objects"),
  "📅": record("calendar", "Objects"),
  "🔒": record("locked", "Objects"),
  "💯": record("hundred_points", "Symbols"),
  "✅": record("check_mark_button", "Symbols"),
  "❌": record("cross_mark", "Symbols"),
  "⚠️": record("warning", "Symbols"),
  "❓": record("red_question_mark", "Symbols"),
  "❗": record("red_exclamation_mark", "Symbols"),
};
