import { describe, expect, it } from "vitest";

import kbTypes from "../../../crates/kb-types/src/lib.rs?raw";
import { API_VERSION } from "./version";

describe("API_VERSION", () => {
  it("equals kb_types::API_VERSION, so the UI and the core agree on the contract", () => {
    const declared = /pub const API_VERSION: u32 = (\d+);/.exec(kbTypes)?.[1];
    expect(declared, "API_VERSION not found in crates/kb-types/src/lib.rs").toBeDefined();
    expect(API_VERSION).toBe(Number(declared));
  });
});
