import { describe, expect, it } from "vitest";

import { DEFAULT_CAPTURE_SHORTCUT, captureKey, keyName } from "./accelerator";
import type { KeyPress } from "./accelerator";

function press(code: string, modifiers: Partial<Omit<KeyPress, "code">> = {}): KeyPress {
  return { code, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...modifiers };
}

describe("keyName", () => {
  it("names a letter, a digit and a function key as the shell does", () => {
    expect(keyName("KeyA")).toBe("A");
    expect(keyName("KeyZ")).toBe("Z");
    expect(keyName("Digit0")).toBe("0");
    expect(keyName("Digit9")).toBe("9");
    expect(keyName("F1")).toBe("F1");
    expect(keyName("F12")).toBe("F12");
    expect(keyName("F24")).toBe("F24");
  });

  it("passes the named keys on", () => {
    for (const code of [
      "Space",
      "Enter",
      "Tab",
      "Backspace",
      "Delete",
      "Insert",
      "Home",
      "End",
      "PageUp",
      "PageDown",
      "ArrowUp",
      "ArrowDown",
      "ArrowLeft",
      "ArrowRight",
      "Escape",
      "Backquote",
      "Backslash",
      "BracketLeft",
      "BracketRight",
      "Comma",
      "Equal",
      "Minus",
      "Period",
      "Quote",
      "Semicolon",
      "Slash",
    ]) {
      expect(keyName(code), code).toBe(code);
    }
  });

  it("has no name for the keys the shell does not accept", () => {
    for (const code of [
      "",
      "Numpad1",
      "NumpadEnter",
      "NumLock",
      "CapsLock",
      "F0",
      "F25",
      "F05",
      "Key1",
      "KeyAB",
      "Digit10",
      "MediaPlayPause",
      "AudioVolumeUp",
      "PrintScreen",
      "ScrollLock",
      "Pause",
      "ContextMenu",
      "IntlBackslash",
      "Unidentified",
      "ControlLeft",
    ]) {
      expect(keyName(code), code).toBeNull();
    }
  });
});

describe("captureKey", () => {
  it("writes the modifiers in the shell's order and then the key", () => {
    expect(captureKey(press("KeyN", { ctrlKey: true, altKey: true }))).toEqual({ kind: "accelerator", accelerator: "Ctrl+Alt+N" });
    expect(captureKey(press("KeyK", { ctrlKey: true, shiftKey: true }))).toEqual({ kind: "accelerator", accelerator: "Ctrl+Shift+K" });
    expect(captureKey(press("Space", { altKey: true }))).toEqual({ kind: "accelerator", accelerator: "Alt+Space" });
    expect(captureKey(press("F5", { metaKey: true }))).toEqual({ kind: "accelerator", accelerator: "Super+F5" });
    expect(captureKey(press("ArrowUp", { ctrlKey: true, altKey: true, shiftKey: true, metaKey: true }))).toEqual({
      kind: "accelerator",
      accelerator: "Ctrl+Alt+Shift+Super+ArrowUp",
    });
  });

  it("reads the physical key, so the layout and Shift do not change it", () => {
    // Shift+1 types "!" and Option+N types a dead tilde: the code is what was pressed.
    expect(captureKey(press("Digit1", { ctrlKey: true, shiftKey: true }))).toEqual({ kind: "accelerator", accelerator: "Ctrl+Shift+1" });
    expect(captureKey(press("KeyN", { altKey: true }))).toEqual({ kind: "accelerator", accelerator: "Alt+N" });
  });

  it("waits while only modifiers are down", () => {
    for (const code of ["ControlLeft", "ControlRight", "AltLeft", "AltRight", "ShiftLeft", "ShiftRight", "MetaLeft", "MetaRight"]) {
      expect(captureKey(press(code, { ctrlKey: true, altKey: true })), code).toEqual({ kind: "pending" });
    }
  });

  it("refuses a key without Ctrl, Alt or Super: it would take the key from every other program", () => {
    expect(captureKey(press("KeyN"))).toEqual({ kind: "noModifier" });
    expect(captureKey(press("F5"))).toEqual({ kind: "noModifier" });
    expect(captureKey(press("Space"))).toEqual({ kind: "noModifier" });
    // Shift alone is not enough either: Shift+A would take every capital A.
    expect(captureKey(press("KeyA", { shiftKey: true }))).toEqual({ kind: "noModifier" });
    expect(captureKey(press("F5", { shiftKey: true }))).toEqual({ kind: "noModifier" });
  });

  it("refuses a key the shell does not accept, whatever the modifiers", () => {
    expect(captureKey(press("Numpad1", { ctrlKey: true }))).toEqual({ kind: "unsupported" });
    expect(captureKey(press("MediaPlayPause", { ctrlKey: true, altKey: true }))).toEqual({ kind: "unsupported" });
    expect(captureKey(press("NumpadEnter"))).toEqual({ kind: "unsupported" });
  });

  it("is the shell's default for the default shortcut", () => {
    expect(captureKey(press("KeyN", { ctrlKey: true, altKey: true }))).toEqual({
      kind: "accelerator",
      accelerator: DEFAULT_CAPTURE_SHORTCUT,
    });
  });
});
