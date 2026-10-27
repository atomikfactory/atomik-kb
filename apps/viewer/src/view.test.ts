// What the viewer shows for each answer of the transport (M13 slice 4), and its words in three languages.

import { describe, expect, it } from "vitest";

import { message, pickLanguage } from "./messages";
import type { MessageKey, ViewerLanguage } from "./messages";
import { nextView } from "./view";
import type { View } from "./view";
import type { OpenedPublication } from "./kb-wasm";
import type { LinkLoad } from "./ViewerTransport";

function publication(title: string, extra: Partial<OpenedPublication> = {}): OpenedPublication {
  return {
    pageId: "page-1",
    title,
    icon: undefined,
    cover: undefined,
    coverY: 50,
    snapshot: () => new Uint8Array(),
    attachmentsJson: () => "[]",
    mentionsJson: () => "[]",
    decryptChunk: () => new Uint8Array(),
    ...extra,
  };
}

const ready = (title: string, pubSeq: number): LinkLoad => ({ status: "ready", publication: publication(title), pubSeq });
const loading: View = { kind: "loading" };

describe("nextView", () => {
  it("shows the page of the first publication, with its icon and cover when it has them", () => {
    const first = nextView(loading, ready("A", 1));
    expect(first).toMatchObject({ kind: "page", revision: 1, offline: false, page: { title: "A", pageId: "page-1" } });
    expect((first as { page: object }).page).not.toHaveProperty("icon");
    expect((first as { page: object }).page).not.toHaveProperty("cover");

    const dressed = nextView(loading, {
      status: "ready",
      pubSeq: 1,
      publication: publication("B", { icon: "🧳", cover: "color:blue", coverY: 30 }),
    });
    expect(dressed).toMatchObject({ page: { icon: "🧳", cover: "color:blue", coverY: 30 } });
  });

  it("draws the page again for every publication handed over, a newer one or the same one once more", () => {
    const first = nextView(loading, ready("A", 1));
    const second = nextView(first, ready("A (edited)", 2));
    const again = nextView(second, { status: "ready", pubSeq: 2, retry: true, publication: publication("A (edited)") });
    expect([first, second, again].map((view) => (view.kind === "page" ? view.revision : 0))).toEqual([1, 2, 3]);
    expect(second).toMatchObject({ page: { title: "A (edited)" } });
  });

  it("replaces the page with 'gone' the moment the link is revoked or expires: nothing of it stays on screen", () => {
    const page = nextView(loading, ready("Secret", 1));
    expect(nextView(page, { status: "gone" })).toEqual({ kind: "gone" });
    expect(nextView(loading, { status: "gone" })).toEqual({ kind: "gone" });
  });

  it("brings the page back when a gone link works again", () => {
    expect(nextView({ kind: "gone" }, ready("A", 1))).toMatchObject({ kind: "page", revision: 1 });
  });

  it("shows 'invalid' for a key that does not open", () => {
    expect(nextView(loading, { status: "invalid" })).toEqual({ kind: "invalid" });
  });

  it("keeps the page and says it is offline when a poll fails, and drops the notice when the server answers", () => {
    const page = nextView(loading, ready("A", 1));
    const offline = nextView(page, { status: "unavailable" });
    expect(offline).toMatchObject({ kind: "page", offline: true });
    expect(nextView(offline, { status: "unavailable" })).toBe(offline);
    expect(nextView(offline, { status: "unchanged" })).toMatchObject({ kind: "page", offline: false });
    expect(nextView(page, { status: "unchanged" })).toBe(page);
    expect(nextView(offline, ready("A", 2))).toMatchObject({ offline: false, revision: 2 });
  });

  it("says the page could not be loaded when the first request fails, and stays gone while offline", () => {
    expect(nextView(loading, { status: "unavailable" })).toEqual({ kind: "failed" });
    expect(nextView({ kind: "failed" }, { status: "unavailable" })).toEqual({ kind: "failed" });
    expect(nextView({ kind: "gone" }, { status: "unavailable" })).toEqual({ kind: "gone" });
  });
});

describe("the viewer's words", () => {
  const languages: ViewerLanguage[] = ["en", "fr", "es"];
  const keys: MessageKey[] = [
    "title.fallback",
    "loading",
    "readOnly",
    "gone.title",
    "gone.body",
    "invalid.title",
    "invalid.body",
    "nokey.title",
    "nokey.body",
    "failed.title",
    "failed.body",
    "retry",
    "offline",
  ];

  it("exist in English, French and Spanish, and the other languages do not copy English", () => {
    for (const key of keys) {
      const english = message("en", key);
      expect(english).not.toBe("");
      for (const language of ["fr", "es"] as const) {
        const text = message(language, key);
        expect(text).not.toBe("");
        expect(text, `${language} ${key}`).not.toBe(english);
      }
    }
    expect(languages).toHaveLength(3);
  });

  it("follow the browser's preferred language, English when none is one of the three", () => {
    expect(pickLanguage(["fr-CA", "en"])).toBe("fr");
    expect(pickLanguage(["de-DE", "es-MX", "fr"])).toBe("es");
    expect(pickLanguage(["EN-gb"])).toBe("en");
    expect(pickLanguage(["de", "ja"])).toBe("en");
    expect(pickLanguage([])).toBe("en");
  });

  it("say the link no longer works, in each language", () => {
    expect(message("en", "gone.title")).toContain("no longer works");
    expect(message("fr", "gone.title")).toContain("ne fonctionne plus");
    expect(message("es", "gone.title")).toContain("ya no funciona");
  });
});
