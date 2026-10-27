// The emoji set behind the `:` picker (emoji-picker.ts), `/emoji` and the search of the icon
// picker (components/IconPicker.tsx): Unicode's emoji with their names and groups, from the
// MIT-licensed `unicode-emoji-json` package (`data-by-emoji.json`, about 390 kB).
//
// The file is loaded with a dynamic `import()` the first time an emoji is looked for: the
// bundler makes it a lazily loaded chunk of the app, never part of the main bundle, and nothing
// is fetched at runtime (the desktop CSP allows only local resources). This module searches; it
// never touches the document: an emoji in text is plain text, nothing about it is stored beyond
// the character.

/** What the data file holds for one emoji (it holds more; this is what is read). */
export interface EmojiRecord {
  /** The Unicode (CLDR) name: `grinning face`. */
  readonly name: string;
  /** The name as a code: `grinning_face`. */
  readonly slug: string;
  /** The Unicode group: `Smileys & Emotion`. */
  readonly group: string;
}

/** The data file: each emoji character with its record. */
export type EmojiData = Readonly<Record<string, EmojiRecord>>;

/** One emoji found by `searchEmoji`. */
export interface EmojiChoice {
  /** The character (or sequence of characters) to put in the text. */
  readonly emoji: string;
  readonly slug: string;
  readonly name: string;
  readonly group: string;
  /** The shortcode to show (`:code:`): the alias or slug that matched best. */
  readonly code: string;
}

/**
 * Shortcodes people know from GitHub and Slack that are not the Unicode slug of their emoji
 * (the others, `fire` and `rocket` among them, are slugs already): each alias with the slug it
 * stands for. An alias that names a slug of another emoji is ignored (the slug wins), and so is
 * one whose slug is not in the data.
 */
export const EMOJI_ALIASES: Readonly<Record<string, string>> = {
  smile: "grinning_face_with_smiling_eyes",
  smiley: "grinning_face_with_big_eyes",
  grin: "beaming_face_with_smiling_eyes",
  laughing: "grinning_squinting_face",
  joy: "face_with_tears_of_joy",
  sob: "loudly_crying_face",
  cry: "crying_face",
  thinking: "thinking_face",
  heart: "red_heart",
  tada: "party_popper",
  "100": "hundred_points",
  check: "check_mark_button",
  white_check_mark: "check_mark_button",
  heavy_check_mark: "check_mark",
  x: "cross_mark",
  bulb: "light_bulb",
  zap: "high_voltage",
  wave: "waving_hand",
  pray: "folded_hands",
  clap: "clapping_hands",
  point_right: "backhand_index_pointing_right",
  point_left: "backhand_index_pointing_left",
  point_up: "backhand_index_pointing_up",
  point_down: "backhand_index_pointing_down",
  question: "red_question_mark",
  exclamation: "red_exclamation_mark",
  lock: "locked",
  "+1": "thumbs_up",
  thumbsup: "thumbs_up",
  "-1": "thumbs_down",
  thumbsdown: "thumbs_down",
};

/** What an empty search lists: eight of the emoji people reach for most, by their shortcodes. */
export const DEFAULT_EMOJI_CODES: readonly string[] = [
  "smile",
  "+1",
  "heart",
  "tada",
  "fire",
  "eyes",
  "rocket",
  "sparkles",
];

interface Entry {
  readonly emoji: string;
  readonly slug: string;
  readonly name: string;
  readonly group: string;
  /** The slug, then the aliases. */
  readonly keys: readonly string[];
  /** The words of the slug and of the name. */
  readonly words: readonly string[];
}

interface EmojiIndex {
  /** In the order of the data (Unicode order: faces first, flags last). */
  readonly entries: readonly Entry[];
  /** Each slug and alias with its emoji. */
  readonly byCode: ReadonlyMap<string, Entry>;
  /** The `DEFAULT_EMOJI_CODES` that the data holds. */
  readonly defaults: readonly EmojiChoice[];
}

/** A skin tone modifier (a variant of an emoji, or the modifier itself). */
const SKIN_TONE_MODIFIER = /[\u{1F3FB}-\u{1F3FF}]/u;
const SKIN_TONE_SLUG = /(?:^|_)skin_tone(?:_|$)/u;

/** Lowercase, without accents. */
function plain(text: string): string {
  return text.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
}

/** The words of a text: runs of letters and digits, lowercase, without accents. */
function wordsOf(text: string): string[] {
  return plain(text)
    .split(/[^a-z0-9]+/u)
    .filter((word) => word !== "");
}

function isRecord(value: unknown): value is EmojiRecord {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const { name, slug, group } = value as Record<string, unknown>;
  return typeof name === "string" && typeof slug === "string" && slug !== "" && typeof group === "string";
}

function choiceOf(entry: Entry, code: string): EmojiChoice {
  return { emoji: entry.emoji, slug: entry.slug, name: entry.name, group: entry.group, code };
}

/**
 * Builds the search index of a data file: one entry per emoji, base emoji only (the data
 * already folds the skin tones of an emoji into one entry; a variant in another file is left
 * out), with the aliases that name one of them.
 */
function buildIndex(data: EmojiData): EmojiIndex {
  const aliasesOf = new Map<string, string[]>();
  for (const [alias, slug] of Object.entries(EMOJI_ALIASES)) {
    aliasesOf.set(slug, [...(aliasesOf.get(slug) ?? []), alias]);
  }
  const slugs = new Set<string>();
  for (const record of Object.values(data)) {
    if (isRecord(record)) {
      slugs.add(record.slug);
    }
  }

  const entries: Entry[] = [];
  for (const [emoji, record] of Object.entries(data)) {
    if (!isRecord(record) || SKIN_TONE_MODIFIER.test(emoji) || SKIN_TONE_SLUG.test(record.slug)) {
      continue;
    }
    const aliases = (aliasesOf.get(record.slug) ?? []).filter((alias) => !slugs.has(alias));
    entries.push({
      emoji,
      slug: record.slug,
      name: record.name,
      group: record.group,
      keys: [record.slug, ...aliases],
      words: [...new Set([...wordsOf(record.slug), ...wordsOf(record.name)])],
    });
  }

  const byCode = new Map<string, Entry>();
  for (const entry of entries) {
    for (const key of entry.keys) {
      if (!byCode.has(key)) {
        byCode.set(key, entry);
      }
    }
  }
  const defaults: EmojiChoice[] = [];
  for (const code of DEFAULT_EMOJI_CODES) {
    const entry = byCode.get(code);
    if (entry !== undefined) {
      defaults.push(choiceOf(entry, code));
    }
  }
  return { entries, byCode, defaults };
}

/** What was typed, as it is compared: no colons around it, lowercase, words joined by `_`. */
function normaliseQuery(query: string): string {
  return plain(query.trim())
    .replace(/^:+|:+$/gu, "")
    .replace(/\s+/gu, "_");
}

/**
 * How well an emoji matches what was typed: the `tier` is 0 when a shortcode is exactly it, 1
 * when a shortcode starts with it, 2 when every word typed starts a word of the name, 3 when a
 * shortcode contains it; the `size` is the length of the shortcode (the shorter the closer), and
 * `code` the one to show. The group is not searched: `:smi` would list every face.
 */
interface Match {
  tier: 0 | 1 | 2 | 3;
  size: number;
  code: string;
}

function matchOf(entry: Entry, query: string, typedWords: readonly string[]): Match | null {
  let best: Match | null = null;
  for (const key of entry.keys) {
    if (key.startsWith(query) && (best === null || key.length < best.size)) {
      best = { tier: key === query ? 0 : 1, size: key.length, code: key };
    }
  }
  if (best !== null) {
    return best;
  }
  if (typedWords.every((typed) => entry.words.some((word) => word.startsWith(typed)))) {
    return { tier: 2, size: entry.slug.length, code: entry.slug };
  }
  const containing = entry.keys.find((key) => key.includes(query));
  return containing === undefined ? null : { tier: 3, size: entry.slug.length, code: containing };
}

function searchIndex(index: EmojiIndex, query: string, limit: number): EmojiChoice[] {
  const wanted = normaliseQuery(query);
  if (limit <= 0) {
    return [];
  }
  if (wanted === "") {
    return index.defaults.slice(0, limit);
  }
  if (!/[a-z0-9]/u.test(wanted)) {
    return []; // `:-)` and `:__` name no emoji; `+1` and `-1` do (they hold a digit)
  }
  // Not `wordsOf`: the `+` of `+1` is part of what was typed, and no word of a name holds it.
  const typedWords = wanted.split("_").filter((word) => word !== "");
  const matches: { entry: Entry; match: Match }[] = [];
  for (const entry of index.entries) {
    const match = matchOf(entry, wanted, typedWords);
    if (match !== null) {
      matches.push({ entry, match });
    }
  }
  return matches
    .sort((a, b) => a.match.tier - b.match.tier || a.match.size - b.match.size) // stable: the rest keeps Unicode order
    .slice(0, limit)
    .map(({ entry, match }) => choiceOf(entry, match.code));
}

let current: EmojiIndex | null = null;
let loading: Promise<void> | null = null;
/** Counts the data swaps, so that a load under way when the data changed is ignored. */
let generation = 0;

/**
 * A test seam: searches `data` instead of the bundled file (`null` brings the file back), so
 * tests never load it. Takes the data at once: nothing is left to load.
 */
export function setEmojiData(data: EmojiData | null): void {
  generation += 1;
  loading = null;
  current = data === null ? null : buildIndex(data);
}

/**
 * Loads the emoji data once and builds the search index. `searchEmoji` loads on its own; call
 * this ahead to have `emojiForCode` work (it answers only from what is loaded). A failed load
 * (a missing chunk) is tried again by the next call.
 */
export function loadEmoji(): Promise<void> {
  if (current !== null) {
    return Promise.resolve();
  }
  if (loading === null) {
    const mine = generation;
    loading = import("unicode-emoji-json/data-by-emoji.json").then(
      (module) => {
        if (mine === generation) {
          current = buildIndex(module.default);
        }
      },
      (error: unknown) => {
        if (mine === generation) {
          loading = null;
        }
        throw error;
      },
    );
  }
  return loading;
}

/** Whether the data is loaded (`emojiForCode` answers only then). */
export function emojiLoaded(): boolean {
  return current !== null;
}

/**
 * The emoji matching what was typed after a colon, best first, at most `limit`: a shortcode
 * that is exactly it, one that starts with it (the shorter first), a name with words that start
 * with each word typed, then a shortcode that contains it. The shortcodes are the
 * Unicode slugs (`grinning_face`) and `EMOJI_ALIASES`; skin tone variants are not listed. Nothing
 * typed gives `DEFAULT_EMOJI_CODES`. Loads the data first; rejects when it cannot be loaded.
 */
export async function searchEmoji(query: string, limit: number): Promise<EmojiChoice[]> {
  await loadEmoji();
  return current === null ? [] : searchIndex(current, query, limit);
}

/**
 * The emoji a shortcode names exactly (a slug or an alias, case aside), or `null` when none
 * does or the data is not loaded yet.
 */
export function emojiForCode(code: string): string | null {
  return current?.byCode.get(code.toLowerCase())?.emoji ?? null;
}
