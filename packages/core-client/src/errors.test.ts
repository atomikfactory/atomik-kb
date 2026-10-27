import { describe, expect, it } from "vitest";

import { CoreError, internalError, isApiError, toCoreError } from "./errors";
import type { ApiError } from "./generated/ApiError";

describe("isApiError", () => {
  it("accepts the serialized ApiError, with and without details", () => {
    expect(isApiError({ code: "NotFound", message: "no such thing" })).toBe(true);
    expect(isApiError({ code: "InvalidInput", message: "bad", details: { field: "name" } })).toBe(true);
  });

  it("accepts every error code of the contract", () => {
    const codes = [
      "NotFound",
      "InvalidInput",
      "AlreadyExists",
      "Storage",
      "Crypto",
      "KeystoreUnavailable",
      "Locked",
      "Internal",
    ];
    for (const code of codes) {
      expect(isApiError({ code, message: "m" }), code).toBe(true);
    }
  });

  it("accepts CoreError instances", () => {
    expect(isApiError(new CoreError({ code: "Storage", message: "disk full" }))).toBe(true);
  });

  it("rejects everything else", () => {
    expect(isApiError(null)).toBe(false);
    expect(isApiError(undefined)).toBe(false);
    expect(isApiError("Internal: boom")).toBe(false);
    expect(isApiError(new Error("plain error"))).toBe(false);
    expect(isApiError({ code: "Internal" })).toBe(false);
    expect(isApiError({ message: "no code" })).toBe(false);
    expect(isApiError({ code: 500, message: "numeric code" })).toBe(false);
    expect(isApiError({ code: "SomethingNew", message: "unknown code" })).toBe(false);
    expect(isApiError({ code: "toString", message: "inherited key is not a code" })).toBe(false);
  });
});

describe("CoreError", () => {
  it("is an Error that carries the ApiError fields", () => {
    const error = new CoreError({ code: "InvalidInput", message: "must not be empty", details: { field: "name" } });
    expect(error).toBeInstanceOf(Error);
    expect(error.name).toBe("CoreError");
    expect(error.message).toBe("must not be empty");
    expect(error.code).toBe("InvalidInput");
    expect(error.details).toEqual({ field: "name" });
    expect(error.stack).toBeTypeOf("string");
  });

  it("serializes to the plain ApiError the core produced", () => {
    const withDetails = new CoreError({ code: "InvalidInput", message: "bad", details: { field: "key" } });
    expect(JSON.parse(JSON.stringify(withDetails))).toEqual({
      code: "InvalidInput",
      message: "bad",
      details: { field: "key" },
    });
    const without: ApiError = new CoreError({ code: "NotFound", message: "gone" }).toApiError();
    expect(without).toEqual({ code: "NotFound", message: "gone" });
    expect("details" in without).toBe(false);
  });

  it("keeps the cause", () => {
    const cause = new TypeError("inner");
    expect(internalError("outer", { cause }).cause).toBe(cause);
  });
});

describe("toCoreError", () => {
  it("returns CoreError instances unchanged", () => {
    const error = new CoreError({ code: "Crypto", message: "wrong key" });
    expect(toCoreError(error)).toBe(error);
  });

  it("keeps code, message and details of a serialized ApiError (what invoke rejects with)", () => {
    const normalised = toCoreError({
      code: "KeystoreUnavailable",
      message: "no usable keystore",
      details: { platform: "linux" },
    });
    expect(normalised).toBeInstanceOf(CoreError);
    expect(normalised.toApiError()).toEqual({
      code: "KeystoreUnavailable",
      message: "no usable keystore",
      details: { platform: "linux" },
    });
  });

  it("turns strings (Tauri IPC failures) into Internal errors", () => {
    const normalised = toCoreError("command dispatch not found");
    expect(normalised.toApiError()).toEqual({ code: "Internal", message: "command dispatch not found" });
  });

  it("turns Error instances into Internal errors and keeps them as the cause", () => {
    const original = new Error("socket closed");
    const normalised = toCoreError(original);
    expect(normalised.code).toBe("Internal");
    expect(normalised.message).toBe("socket closed");
    expect(normalised.cause).toBe(original);
  });

  it("does not trust a code this build does not know, but keeps it readable", () => {
    const normalised = toCoreError({ code: "RateLimited", message: "slow down" });
    expect(normalised.toApiError()).toEqual({ code: "Internal", message: "RateLimited: slow down" });
  });

  it("falls back to a generic Internal error for anything else", () => {
    for (const value of [undefined, null, 42, "", {}, [], Symbol("x")]) {
      const normalised = toCoreError(value);
      expect(normalised.code).toBe("Internal");
      expect(normalised.message).toBe("Unknown error");
    }
  });
});
