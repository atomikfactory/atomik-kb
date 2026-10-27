import { describe, expect, it } from "vitest";

import { CoreError } from "./errors";
import {
  COVER_Y_DEFAULT,
  COVER_Y_MAX,
  MAX_INVITE_TOKEN_CHARS,
  MAX_SERVER_URL_CHARS,
  PAIRING_CODE_LEN,
  cover,
  coverAttachment,
  coverY,
  displayName,
  formatPairingCode,
  icon,
  iconAttachment,
  invalid,
  inviteToken,
  kitText,
  pageTitle,
  pairingCode,
  passphrase,
  position,
  serverUrl,
  spaceIcon,
} from "./validate";

function refusal(run: () => unknown): CoreError {
  try {
    run();
  } catch (error) {
    expect(error).toBeInstanceOf(CoreError);
    return error as CoreError;
  }
  throw new Error("expected the call to throw");
}

describe("invalid", () => {
  it("is an InvalidInput error that names the field", () => {
    expect(invalid("title", "too long").toApiError()).toEqual({
      code: "InvalidInput",
      message: "too long",
      details: { field: "title" },
    });
  });
});

describe("displayName", () => {
  it("trims and requires 1 to max characters without control characters", () => {
    expect(displayName("  Work  ", 10, "name")).toBe("Work");
    expect(refusal(() => displayName("   ", 10, "name")).message).toBe("must not be empty");
    expect(refusal(() => displayName("12345678901", 10, "name")).message).toBe("must be at most 10 characters");
    expect(refusal(() => displayName("a\tb", 10, "name")).message).toBe("must not contain control characters");
    expect(refusal(() => displayName("", 10, "firstSpaceName")).toApiError().details).toEqual({
      field: "firstSpaceName",
    });
  });

  it("counts characters, not UTF-16 code units", () => {
    expect(displayName("\u{1F600}".repeat(10), 10, "name")).toBe("\u{1F600}".repeat(10));
    expect(refusal(() => displayName("\u{1F600}".repeat(11), 10, "name")).message).toBe(
      "must be at most 10 characters",
    );
  });
});

describe("pageTitle", () => {
  it("allows an empty title and up to 500 characters", () => {
    expect(pageTitle("   ")).toBe("");
    expect(pageTitle(" Plan ")).toBe("Plan");
    expect(pageTitle("x".repeat(500))).toHaveLength(500);
    expect(refusal(() => pageTitle("x".repeat(501))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "must be at most 500 characters",
      details: { field: "title" },
    });
  });

  it("refuses control characters, so no line breaks", () => {
    expect(refusal(() => pageTitle("a\nb")).message).toBe("must not contain control characters");
    expect(refusal(() => pageTitle("a\u0000b")).message).toBe("must not contain control characters");
    // Surrounding whitespace is trimmed first, so a trailing newline is fine.
    expect(pageTitle("Plan\n")).toBe("Plan");
  });
});

describe("icon", () => {
  it("treats null and undefined as no icon", () => {
    expect(icon(null)).toBeUndefined();
    expect(icon(undefined)).toBeUndefined();
  });

  it("trims, and requires 1 to 32 UTF-8 bytes without control characters", () => {
    expect(icon(" 📚 ")).toBe("📚");
    expect(icon("🙂".repeat(8))).toBe("🙂".repeat(8));
    expect(refusal(() => icon("   ")).toApiError()).toEqual({
      code: "InvalidInput",
      message: "must not be empty; omit it to clear",
      details: { field: "icon" },
    });
    expect(refusal(() => icon("🙂".repeat(9))).message).toBe("must be at most 32 bytes");
    expect(refusal(() => icon("a".repeat(33))).message).toBe("must be at most 32 bytes");
    expect(refusal(() => icon("a\u0007")).message).toBe("must not contain control characters");
  });

  it("accepts the emoji and short text of before, and only the prefixes are special (case-sensitively)", () => {
    for (const ok of ["🌱", "A", "Notes", "icons:book", "attachments", "Icon:book", "ATTACHMENT:x"]) {
      expect(icon(ok), ok).toBe(ok);
    }
    expect(icon("a".repeat(32))).toBe("a".repeat(32));
  });
});

/** The refusal of an icon: `InvalidInput` for the field `icon`; returns the message. */
function iconRefusal(raw: string): string {
  const error = refusal(() => icon(raw));
  expect(error.toApiError().code, raw).toBe("InvalidInput");
  expect(error.toApiError().details, raw).toEqual({ field: "icon" });
  return error.message;
}

const UUID = "5f0c1d2e-3a4b-4c5d-8e6f-0a1b2c3d4e5f";

describe("icon of the bundled set (icon:<name>[:<color>])", () => {
  it("accepts the grammar and trims", () => {
    for (const ok of [
      "icon:a",
      "icon:book-open",
      "icon:book-open:blue",
      "icon:0",
      "icon:a-b-c-9:gray",
      `icon:${"x".repeat(48)}`,
      `icon:${"x".repeat(48)}:purple`,
    ]) {
      expect(icon(ok), ok).toBe(ok);
    }
    for (const color of ["gray", "brown", "orange", "yellow", "green", "blue", "purple", "pink", "red"]) {
      expect(icon(`icon:star:${color}`), color).toBe(`icon:star:${color}`);
    }
    expect(icon("  icon:book-open:blue ")).toBe("icon:book-open:blue");
  });

  it("tells every failing rule apart, with the messages of kb-core", () => {
    const missing = iconRefusal("icon:");
    expect(missing).toBe("an icon of the set is icon:<name> or icon:<name>:<color>; the name is missing");
    expect(iconRefusal("icon::blue")).toBe(missing);
    const characters = iconRefusal("icon:Book");
    expect(characters).toBe("the icon name may only contain a-z, 0-9 and '-'");
    for (const bad of ["icon:book open", "icon:book_open", "icon:livreé"]) {
      expect(iconRefusal(bad), bad).toBe(characters);
    }
    const long = iconRefusal(`icon:${"x".repeat(49)}`);
    expect(long).toBe("the icon name must be at most 48 characters");
    const dash = iconRefusal("icon:-book");
    expect(dash).toBe("the icon name must not start or end with '-'");
    expect(iconRefusal("icon:book-")).toBe(dash);
    expect(iconRefusal("icon:-")).toBe(dash);
    const colour = iconRefusal("icon:book:teal");
    expect(colour).toBe(
      "the icon colour must be one of: gray, brown, orange, yellow, green, blue, purple, pink, red; omit it for the text colour",
    );
    for (const bad of ["icon:book:", "icon:book:Blue", "icon:book:blue:red"]) {
      expect(iconRefusal(bad), bad).toBe(colour);
    }
    expect(new Set([missing, characters, long, dash, colour]).size).toBe(5);
  });
});

describe("icon of an uploaded image (attachment:<id>)", () => {
  it("needs a lowercase hyphenated UUID", () => {
    expect(icon(`attachment:${UUID}`)).toBe(`attachment:${UUID}`);
    expect(icon(` attachment:${UUID} `)).toBe(`attachment:${UUID}`);
    const message = "the attachment id must be a lowercase hyphenated UUID";
    for (const bad of [
      "attachment:",
      "attachment:abc",
      `attachment:${UUID.toUpperCase()}`,
      `attachment:${UUID.replaceAll("-", "")}`,
      `attachment:${UUID}0`,
      `attachment:${UUID.slice(1)}`,
      `attachment:${UUID.slice(0, 35)}g`,
      `attachment:{${UUID}}`,
      `attachment: ${UUID}`,
      `attachment:${UUID}:blue`,
    ]) {
      expect(iconRefusal(bad), bad).toBe(message);
    }
  });

  it("iconAttachment reads the id out of an icon and nothing else", () => {
    expect(iconAttachment(`attachment:${UUID}`)).toBe(UUID);
    for (const other of [undefined, "", "🌱", "icon:book", "attachment:nope", `attachment:${UUID.toUpperCase()}`]) {
      expect(iconAttachment(other), String(other)).toBeUndefined();
    }
  });
});

describe("cover (ADR-0040)", () => {
  const UUID = "5f0c1d2e-3a4b-4c5d-8e6f-0a1b2c3d4e5f";
  const field = (run: () => unknown): [string | undefined, string] => {
    const error = refusal(run);
    return [(error.toApiError().details as { field?: string } | undefined)?.field, error.message];
  };

  it("clears with null or undefined and trims", () => {
    expect(cover(null)).toBeUndefined();
    expect(cover(undefined)).toBeUndefined();
    expect(cover(" gradient:dawn ")).toBe("gradient:dawn");
  });

  it("accepts an image, a palette colour and a named gradient", () => {
    for (const ok of [`attachment:${UUID}`, "color:gray", "color:red", "gradient:a", "gradient:blue-2", `gradient:${"x".repeat(32)}`]) {
      expect(cover(ok)).toBe(ok);
    }
    expect(coverAttachment(`attachment:${UUID}`)).toBe(UUID);
    expect(coverAttachment("color:blue")).toBeUndefined();
    expect(coverAttachment(undefined)).toBeUndefined();
    expect(coverAttachment("attachment:nope")).toBeUndefined();
  });

  it("tells each refusal apart, on the field cover", () => {
    expect(field(() => cover(""))).toEqual(["cover", "must not be empty; omit it to clear"]);
    expect(field(() => cover("🌱"))).toEqual(["cover", "a cover is attachment:<id>, color:<name> or gradient:<name>"]);
    expect(field(() => cover("attachment:abc"))).toEqual(["cover", "the attachment id must be a lowercase hyphenated UUID"]);
    expect(field(() => cover("color:teal"))[1]).toContain("the cover colour must be one of: gray, brown");
    expect(field(() => cover("gradient:"))[1]).toBe("a gradient cover is gradient:<name>; the name is missing");
    expect(field(() => cover("gradient:Sun"))[1]).toBe("the gradient name may only contain a-z, 0-9 and '-'");
    expect(field(() => cover(`gradient:${"x".repeat(33)}`))[1]).toBe("the gradient name must be at most 32 characters");
  });

  it("takes a position from 0 to 100, none being the default", () => {
    expect(coverY(undefined)).toBeUndefined();
    expect(coverY(null)).toBeUndefined();
    for (const ok of [0, 50, 100]) {
      expect(coverY(ok)).toBe(ok);
    }
    for (const bad of [-1, 101, 2.5, Number.NaN]) {
      expect(field(() => coverY(bad))).toEqual(["coverY", "the cover position must be between 0 and 100"]);
    }
    expect([COVER_Y_DEFAULT, COVER_Y_MAX]).toEqual([50, 100]);
  });
});

describe("spaceIcon", () => {
  it("is the icon grammar without the uploaded image", () => {
    expect(spaceIcon(null)).toBeUndefined();
    expect(spaceIcon(undefined)).toBeUndefined();
    expect(spaceIcon(" 💼 ")).toBe("💼");
    expect(spaceIcon("icon:briefcase:brown")).toBe("icon:briefcase:brown");
    const image = refusal(() => spaceIcon(`attachment:${UUID}`));
    expect(image.toApiError()).toEqual({
      code: "InvalidInput",
      message: "a space icon cannot be an image",
      details: { field: "icon" },
    });
    // A malformed image icon is told apart from a well-formed one a space may not have.
    expect(refusal(() => spaceIcon("attachment:x")).message).toBe(
      "the attachment id must be a lowercase hyphenated UUID",
    );
    expect(() => spaceIcon("")).toThrow();
    expect(() => spaceIcon("a".repeat(33))).toThrow();
  });
});

describe("position", () => {
  it("accepts whole numbers from zero, however large", () => {
    expect(position(0)).toBe(0);
    expect(position(7)).toBe(7);
    expect(position(Number.MAX_SAFE_INTEGER)).toBe(Number.MAX_SAFE_INTEGER);
  });

  it("refuses everything a u32 cannot hold", () => {
    for (const bad of [-1, 0.5, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(refusal(() => position(bad)).toApiError(), String(bad)).toEqual({
        code: "InvalidInput",
        message: "must be a non-negative integer",
        details: { field: "index" },
      });
    }
  });
});

describe("passphrase", () => {
  it("accepts 8 to 256 UTF-8 bytes, exactly as typed", () => {
    expect(passphrase("12345678")).toBe("12345678");
    expect(passphrase("x".repeat(256))).toHaveLength(256);
    // Never trimmed: surrounding spaces are part of the secret.
    expect(passphrase("  spaced  ")).toBe("  spaced  ");
  });

  it("counts bytes, not characters", () => {
    expect(passphrase("éééé")).toBe("éééé");
    expect(() => passphrase("€€")).toThrow("passphrase must be 8 to 256 bytes");
    expect(() => passphrase("€".repeat(86))).toThrow("passphrase must be 8 to 256 bytes");
  });

  it("refuses shorter and longer ones like the core, for the field passphrase", () => {
    for (const bad of ["", "1234567", "x".repeat(257)]) {
      expect(refusal(() => passphrase(bad)).toApiError(), String(bad.length)).toEqual({
        code: "InvalidInput",
        message: "passphrase must be 8 to 256 bytes",
        details: { field: "passphrase" },
      });
    }
  });
});

describe("kitText", () => {
  it("trims and requires 1 to 1024 bytes", () => {
    expect(kitText("  abandon ability \n")).toBe("abandon ability");
    expect(kitText("a".repeat(1024))).toHaveLength(1024);
    expect(refusal(() => kitText(" \n\t ")).toApiError()).toEqual({
      code: "InvalidInput",
      message: "must not be empty",
      details: { field: "kit" },
    });
    expect(refusal(() => kitText("a".repeat(1025))).message).toBe("must be at most 1024 bytes");
  });
});

describe("serverUrl", () => {
  const BAD_URL = {
    code: "InvalidInput",
    message: "the server address must be http(s)://host[:port]",
    details: { field: "serverUrl" },
  };

  it("accepts http and https with a host, an optional port and an optional path, trimmed", () => {
    for (const good of [
      "http://localhost:8787",
      "https://sync.example.com",
      "https://sync.example.com/",
      "https://example.com/kb",
      "HTTPS://Sync.Example.com:443",
      "http://192.168.1.20:8787",
      "http://[::1]:8787",
    ]) {
      expect(serverUrl(good), good).toBe(good);
    }
    expect(serverUrl("  https://sync.example.com \n")).toBe("https://sync.example.com");
  });

  it("refuses anything else for the field serverUrl, with the core's message", () => {
    for (const bad of [
      "",
      "   ",
      "sync.example.com",
      "localhost:8787",
      "ftp://sync.example.com",
      "ws://sync.example.com",
      "file:///etc/passwd",
      "http:sync.example.com",
      "http:/sync.example.com",
      "http://",
      "https:///sync.example.com",
      "http://user:secret@sync.example.com",
      "http://user@sync.example.com",
      "http://@sync.example.com",
      "https://sync.example.com?token=1",
      "https://sync.example.com/?",
      "https://sync.example.com#top",
      "https://sync.example.com/#",
      "https://sync example.com",
      "https://sync.example.com/a b",
      "https://sync.example.com\t/x",
      "http://sync.example.com:99999",
      "http://sync.example.com:port",
      "http://exa%mple.com",
      "https://sync.example.com/me@home",
      "http://\\sync.example.com",
    ]) {
      expect(refusal(() => serverUrl(bad)).toApiError(), bad).toEqual(BAD_URL);
    }
  });

  it("allows at most 2048 characters after trimming", () => {
    const prefix = "https://sync.example.com/";
    const longest = prefix + "a".repeat(MAX_SERVER_URL_CHARS - prefix.length);
    expect(serverUrl(` ${longest} `)).toBe(longest);
    expect(refusal(() => serverUrl(`${longest}a`)).toApiError()).toEqual(BAD_URL);
  });
});

describe("inviteToken", () => {
  const BAD_TOKEN = {
    code: "InvalidInput",
    message: "the invite token must be 1 to 128 printable ASCII characters",
    details: { field: "inviteToken" },
  };

  it("trims and accepts 1 to 128 printable ASCII characters", () => {
    expect(inviteToken("x")).toBe("x");
    expect(inviteToken("  invite-2026_10.04~ok \n")).toBe("invite-2026_10.04~ok");
    expect(inviteToken("two words")).toBe("two words");
    expect(inviteToken("!".repeat(MAX_INVITE_TOKEN_CHARS))).toHaveLength(128);
  });

  it("refuses empty, longer and non-ASCII tokens without quoting them", () => {
    for (const bad of ["", " \t\n ", "x".repeat(129), "tokén", "tok\u0000en", "tok\u007fen", "tok\u{1F511}"]) {
      const error = refusal(() => inviteToken(bad)).toApiError();
      expect(error, JSON.stringify(bad)).toEqual(BAD_TOKEN);
    }
  });
});

describe("pairingCode", () => {
  const BAD_CODE = { code: "InvalidInput", message: "the code is 12 letters and digits", details: { field: "code" } };

  it("removes spaces and dashes, takes any case and reads O as 0 and I or L as 1", () => {
    expect(pairingCode("ABCD EFGH JKMN")).toBe("ABCDEFGHJKMN");
    expect(pairingCode(" abcd-efgh-jkmn ")).toBe("ABCDEFGHJKMN");
    expect(pairingCode("0123\t4567\n89xz")).toBe("0123456789XZ");
    expect(pairingCode("OIL0 il11 oooo")).toBe("011011110000");
    expect(pairingCode("pqrs tvwx yz00")).toBe("PQRSTVWXYZ00");
  });

  it("refuses other characters and other lengths for the field code", () => {
    for (const bad of ["", "ABCD EFGH JKM", "ABCD EFGH JKMNP", "ABCD EFGH JKMU", "ABCD_EFGH_JKMN", "ABCD EFGH JKMé", "ßBCD EFGH JKMN"]) {
      expect(refusal(() => pairingCode(bad)).toApiError(), bad).toEqual(BAD_CODE);
    }
  });

  it("formatPairingCode shows the canonical code in three groups of four", () => {
    expect(formatPairingCode("abcdefghjkmn")).toBe("ABCD EFGH JKMN");
    expect(formatPairingCode("o1l0-ABCD-efgh")).toBe("0110 ABCD EFGH");
    expect(refusal(() => formatPairingCode("short")).toApiError()).toEqual(BAD_CODE);
    expect(PAIRING_CODE_LEN).toBe(12);
  });
});
