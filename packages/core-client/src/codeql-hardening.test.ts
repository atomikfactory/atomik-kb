import { describe, expect, it } from "vitest";
import { isBase64 } from "./comments";
import { htmlText, parseNumber } from "./memory-import-text";
import { newSessionId } from "./dictation";
import { sanitisedName } from "./shell-names";
import { trimEndChars } from "./trim-end";

const BIG = 50_000;

function timed(run: () => void): number {
  const start = performance.now();
  run();
  return performance.now() - start;
}

describe("trimEndChars", () => {
  it("drops only the trailing run", () => {
    expect(trimEndChars("a=b==", "=")).toBe("a=b");
    expect(trimEndChars("///", "/")).toBe("");
    expect(trimEndChars("abc", "=")).toBe("abc");
    expect(trimEndChars("a. .", ". ")).toBe("a");
  });
  it("is linear on a long run that does not end the text", () => {
    expect(timed(() => trimEndChars(`${"=".repeat(BIG)}x`, "="))).toBeLessThan(200);
  });
});

describe("regexes that were quadratic", () => {
  it("isBase64 stays fast", () => {
    expect(timed(() => isBase64(`${"=".repeat(BIG)}x`))).toBeLessThan(200);
    expect(isBase64("QUJD")).toBe(true);
    expect(isBase64("QQ==")).toBe(true);
    expect(isBase64("Q===")).toBe(false);
  });
  it("sanitisedName stays fast", () => {
    expect(timed(() => sanitisedName(`${" ".repeat(BIG)}x`))).toBeLessThan(200);
    expect(sanitisedName("a. . ")).toBe("a");
  });
  it("parseNumber stays fast and keeps its grammar", () => {
    expect(timed(() => parseNumber(`${"00".repeat(BIG)}x`))).toBeLessThan(200);
    for (const ok of ["1", "-1.5", "1.", ".5", "+2e10", "1.5E-3"]) {
      expect(parseNumber(ok)).toBe(Number(ok));
    }
    for (const bad of [".", "1e", "1,000", "--1", "1.2.3"]) {
      expect(parseNumber(bad)).toBeUndefined();
    }
  });
});

describe("htmlText leaves no tag behind", () => {
  it("strips nested/reassembled markup", () => {
    const { paragraphs } = htmlText("<body><p>a <scr<script>x</script>ipt>alert(1)</p><p>b <!-<!-- x -->- c --></p></body>");
    const text = paragraphs.join("\n");
    expect(text).not.toMatch(/<\s*script/iu);
    expect(text).not.toContain("<!--");
  });
  it("keeps ordinary text", () => {
    expect(htmlText("<body><p>Hello <b>world</b></p><p>two</p></body>").paragraphs).toEqual(["Hello world", "two"]);
  });
  it("is fast on adversarial input", () => {
    expect(timed(() => htmlText(`<body>${"<".repeat(BIG)}</body>`))).toBeLessThan(1000);
  });
});

describe("newSessionId", () => {
  it("is valid and unique without randomUUID", () => {
    const original: unknown = Reflect.get(globalThis.crypto, "randomUUID");
    Object.defineProperty(globalThis.crypto, "randomUUID", { value: undefined, configurable: true });
    try {
      const a = newSessionId();
      const b = newSessionId();
      expect(a).toMatch(/^[A-Za-z0-9_-]{1,64}$/u);
      expect(a).not.toBe(b);
    } finally {
      Object.defineProperty(globalThis.crypto, "randomUUID", { value: original, configurable: true });
    }
  });
});
