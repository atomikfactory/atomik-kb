import { describe, expect, it } from "vitest";

import { PAIRING_CODE_CHARS, digitGroups, normalizePairingCode, pairingCodeGroups } from "./pairing";

describe("normalizePairingCode", () => {
  it("drops separators and upper-cases, however the code was typed", () => {
    for (const typed of ["ABCD EFGH JKMN", "abcd-efgh-jkmn", " abcd efgh\tjkmn ", "ab.cd_ef-gh jk mn", "ABCDEFGHJKMN"]) {
      expect(normalizePairingCode(typed), typed).toBe("ABCDEFGHJKMN");
    }
    expect(normalizePairingCode("ABCDEFGHJKMN")).toHaveLength(PAIRING_CODE_CHARS);
    expect(normalizePairingCode("")).toBe("");
  });
});

describe("pairingCodeGroups", () => {
  it("shows the code in three groups of four", () => {
    expect(pairingCodeGroups("ABCD EFGH JKMN")).toEqual(["ABCD", "EFGH", "JKMN"]);
    expect(pairingCodeGroups("abcdefghjkmn")).toEqual(["ABCD", "EFGH", "JKMN"]);
  });
});

describe("digitGroups", () => {
  it("splits six digits into two groups of three, and leaves anything else whole", () => {
    expect(digitGroups("428516")).toEqual(["428", "516"]);
    expect(digitGroups("1234")).toEqual(["1234"]);
  });
});
