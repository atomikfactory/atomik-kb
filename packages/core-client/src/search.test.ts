import { describe, expect, it } from "vitest";

import type { TextRange } from "./generated/TextRange";
import { foldText, matchText, parseQuery } from "./search";

/** The snippet of a match and its highlighted parts, for readable assertions. */
function marked(text: string, query: string): { snippet: string; parts: string[] } | undefined {
  const match = matchText(text, parseQuery(query));
  if (match === undefined) {
    return undefined;
  }
  return {
    snippet: match.snippet,
    parts: match.highlights.map((range: TextRange) => match.snippet.slice(range.start, range.end)),
  };
}

describe("foldText", () => {
  it("ignores case and diacritics", () => {
    expect(foldText("Café ÉLÈVE Ñandú Straße İstanbul")).toBe("cafe eleve nandu straße istanbul");
    expect(foldText("é")).toBe("e");
  });
});

describe("parseQuery (kb-storage's FTS5 query rules)", () => {
  it("separates words on whitespace, control characters and FTS5 syntax", () => {
    expect(parseQuery("Hello  wörld")).toEqual([["hello"], ["world"]]);
    expect(parseQuery("atomik-kb")).toEqual([["atomik"], ["kb"]]);
    expect(parseQuery('"quoted phrase"')).toEqual([["quoted"], ["phrase"]]);
    expect(parseQuery("-a +b ^c d*")).toEqual([["a"], ["b"], ["c"], ["d"]]);
    expect(parseQuery("title:foo (x)")).toEqual([["title"], ["foo"], ["x"]]);
    expect(parseQuery("a\u0000b")).toEqual([["a"], ["b"]]);
  });

  it("drops the bare operators in capitals only", () => {
    expect(parseQuery("a AND b OR c NOT d NEAR e")).toEqual([["a"], ["b"], ["c"], ["d"], ["e"]]);
    expect(parseQuery("and or not near")).toEqual([["and"], ["or"], ["not"], ["near"]]);
    expect(parseQuery("ANDROID NOTE")).toEqual([["android"], ["note"]]);
  });

  it("keeps other punctuation inside a word, which makes it a phrase", () => {
    expect(parseQuery("a.b")).toEqual([["a", "b"]]);
    expect(parseQuery("NEAR/2")).toEqual([["near", "2"]]);
  });

  it("drops words without letters or digits", () => {
    expect(parseQuery("& && ... ?! @ $% … \u{1F389}")).toEqual([]);
    expect(parseQuery("plain & words")).toEqual([["plain"], ["words"]]);
    expect(parseQuery("日本語 x2")).toEqual([["日本語"], ["x2"]]);
  });
});

describe("matchText", () => {
  it("needs every word, each as a token prefix, ignoring case and diacritics", () => {
    expect(marked("Notes about the Café", "cafe not")).toEqual({
      snippet: "Notes about the Café",
      parts: ["Notes", "Café"],
    });
    expect(marked("Notes about the Café", "otes")).toBeUndefined();
    expect(marked("Notes about the Café", "notes tea")).toBeUndefined();
    expect(marked("Planning", "plan")?.parts).toEqual(["Planning"]);
    expect(marked("atomik and kb", "atomik-kb")?.parts).toEqual(["atomik", "kb"]);
  });

  it("matches a phrase as consecutive tokens, the last one as a prefix", () => {
    expect(marked("see v1.2 notes", "v1.2")?.parts).toEqual(["v1.2"]);
    expect(marked("see v1 then 2", "v1.2")).toBeUndefined();
    expect(marked("version 10.25 out", "10.2")?.parts).toEqual(["10.25"]);
  });

  it("highlights every match inside the snippet and coalesces overlapping ones", () => {
    expect(marked("red green red", "red")?.parts).toEqual(["red", "red"]);
    expect(marked("one two three", "one.two two")?.parts).toEqual(["one two"]);
  });

  it("chooses 12 tokens like FTS5: matches centred, leading and trailing text where it reaches the ends", () => {
    const words = Array.from({ length: 30 }, (_, i) => `w${String(i)}`);
    const long = `${words.join(" ")}.`;
    expect(marked(long, "w20")).toEqual({ snippet: `…${words.slice(15, 27).join(" ")}…`, parts: ["w20"] });
    // w1 and w10 to w19 all match; the window at the start of the text wins.
    expect(marked(long, "w1")?.snippet).toBe(`${words.slice(0, 12).join(" ")}…`);
    expect(marked(long, "w29")?.snippet).toBe(`…${words.slice(18).join(" ")}.`);
    expect(marked("(short) text!", "text")).toEqual({ snippet: "(short) text!", parts: ["text"] });
  });

  it("prefers a snippet that starts the sentence holding the match", () => {
    const text =
      "Alpha beta gamma delta epsilon zeta eta theta iota kappa. " +
      "Lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega.";
    expect(marked(text, "sigma")).toEqual({
      snippet: "…Lambda mu nu xi omicron pi rho sigma tau upsilon phi chi…",
      parts: ["sigma"],
    });
  });

  it("prefers the window holding the most different words", () => {
    const filler = (n: number): string => Array.from({ length: n }, (_, i) => `f${String(i)}`).join(" ");
    const text = `apple ${filler(20)} apple banana ${filler(20)}`;
    expect(marked(text, "apple banana")?.parts).toEqual(["apple", "banana"]);
  });

  it("reports highlights in UTF-16 code units", () => {
    // 👩‍💻 is five code units (two surrogate pairs and a joiner), each 𝔘𝔫𝔦 letter two.
    const match = matchText("👩‍💻 𝔘𝔫𝔦 café", parseQuery("cafe"));
    expect(match?.highlights).toEqual([{ start: 13, end: 17 }]);
    const fraktur = matchText("👩‍💻 𝔘𝔫𝔦 café", parseQuery("𝔘𝔫"));
    expect(fraktur?.highlights).toEqual([{ start: 6, end: 12 }]);
  });

  it("matches nothing for an empty query or an empty text", () => {
    expect(matchText("anything", [])).toBeUndefined();
    expect(matchText("", [["a"]])).toBeUndefined();
  });
});
