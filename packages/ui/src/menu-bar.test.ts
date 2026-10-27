// The lone Alt press that shows the desktop shell's menu bar (ADR-0040 item 12): the pure detector over many key
// sequences, and the rule for what sends the shown bar away again. The whole-app behaviour is `menu-bar-app.test.tsx`.

import { describe, expect, it } from "vitest";

import { altKeyInput, hidesMenuBar, loneAlt } from "./menu-bar";
import type { AltInput, AltKeyInput, AltState } from "./menu-bar";

type Mods = { ctrl?: boolean; meta?: boolean; shift?: boolean; composing?: boolean; repeat?: boolean };

const keyInput = (type: AltKeyInput["type"], key: string, mods: Mods): AltKeyInput => ({
  type,
  key,
  repeat: mods.repeat ?? false,
  ctrlKey: mods.ctrl ?? false,
  metaKey: mods.meta ?? false,
  shiftKey: mods.shift ?? false,
  isComposing: mods.composing ?? false,
});
const down = (key: string, mods: Mods = {}): AltKeyInput => keyInput("keydown", key, mods);
const up = (key: string, mods: Mods = {}): AltKeyInput => keyInput("keyup", key, mods);
const POINTER: AltInput = { type: "pointer" };
const BLUR: AltInput = { type: "blur" };

/** How many times the sequence completes a lone Alt press. */
function fires(sequence: readonly AltInput[], start: AltState = "idle"): number {
  let state = start;
  let count = 0;
  for (const input of sequence) {
    const step = loneAlt(state, input);
    state = step.state;
    count += step.fire ? 1 : 0;
  }
  return count;
}

describe("a lone Alt press", () => {
  it("is Alt down and Alt up, and fires on the release", () => {
    expect(fires([down("Alt"), up("Alt")])).toBe(1);
    // Not on the press.
    expect(loneAlt("idle", down("Alt"))).toEqual({ state: "armed", fire: false });
    expect(loneAlt("armed", up("Alt"))).toEqual({ state: "idle", fire: true });
  });

  it("stays one while the key repeats", () => {
    expect(fires([down("Alt"), down("Alt", { repeat: true }), down("Alt", { repeat: true }), up("Alt")])).toBe(1);
  });

  it("fires again for each press", () => {
    expect(fires([down("Alt"), up("Alt"), down("Alt"), up("Alt"), down("Alt"), up("Alt")])).toBe(3);
  });

  it("works for either Alt key (the page reads both as Alt)", () => {
    expect(fires([down("Alt"), up("Alt")])).toBe(1);
    expect(fires([{ ...down("Alt") }, { ...up("Alt") }])).toBe(1);
  });

  it("is not a press if the release has no press (focus came back with Alt held)", () => {
    expect(fires([up("Alt")])).toBe(0);
    expect(fires([up("Alt"), up("Alt")])).toBe(0);
  });
});

describe("what is not a lone Alt press", () => {
  it("Alt with an arrow: back and forward (Alt+Left, Alt+Right) and word movement in the editor", () => {
    for (const key of ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"]) {
      expect(fires([down("Alt"), down(key), up(key), up("Alt")]), key).toBe(0);
    }
  });

  it("Alt with any key, whichever order they are released in", () => {
    expect(fires([down("Alt"), down("f"), up("f"), up("Alt")])).toBe(0);
    expect(fires([down("Alt"), down("f"), up("Alt"), up("f")])).toBe(0);
    expect(fires([down("Alt"), down("Tab"), up("Tab"), up("Alt")])).toBe(0);
    expect(fires([down("Alt"), down(" "), up(" "), up("Alt")])).toBe(0);
    expect(fires([down("Alt"), down("F4"), up("F4"), up("Alt")])).toBe(0);
  });

  it("Ctrl+Alt+N, the quick capture's own chord, however it is pressed and released", () => {
    // Ctrl first: Alt arrives with ctrlKey set.
    expect(
      fires([down("Control", { ctrl: true }), down("Alt", { ctrl: true }), down("n", { ctrl: true }), up("n", { ctrl: true }), up("Alt", { ctrl: true }), up("Control")]),
    ).toBe(0);
    // Alt first, then Ctrl, then N.
    expect(fires([down("Alt"), down("Control", { ctrl: true }), down("n", { ctrl: true }), up("n"), up("Control"), up("Alt")])).toBe(0);
    // Ctrl released before Alt.
    expect(fires([down("Control", { ctrl: true }), down("Alt", { ctrl: true }), up("Control"), up("Alt")])).toBe(0);
  });

  it("Alt held after a chord, with the key repeating once the other modifier is gone", () => {
    // Ctrl+Alt, Ctrl released, Alt still down and repeating, then released: the chord stays a chord.
    expect(
      fires([down("Control", { ctrl: true }), down("Alt", { ctrl: true }), up("Control"), down("Alt", { repeat: true }), up("Alt")]),
    ).toBe(0);
  });

  it("AltGr (Ctrl+Alt on Windows, `AltGraph` for the page), which types characters", () => {
    expect(fires([down("Control", { ctrl: true }), down("AltGraph", { ctrl: true }), up("AltGraph", { ctrl: true }), up("Control")])).toBe(0);
    // Even a layout that reports only the Alt key with a Ctrl.
    expect(fires([down("Alt", { ctrl: true }), up("Alt", { ctrl: true })])).toBe(0);
    expect(fires([down("AltGraph"), up("AltGraph")])).toBe(0);
  });

  it("Shift+Alt (the layout switch) and Meta+Alt", () => {
    expect(fires([down("Shift", { shift: true }), down("Alt", { shift: true }), up("Alt", { shift: true }), up("Shift")])).toBe(0);
    expect(fires([down("Alt"), down("Shift", { shift: true }), up("Shift"), up("Alt")])).toBe(0);
    expect(fires([down("Meta", { meta: true }), down("Alt", { meta: true }), up("Alt", { meta: true }), up("Meta")])).toBe(0);
  });

  it("two Alt keys together", () => {
    expect(fires([down("Alt"), down("Alt"), up("Alt"), up("Alt")])).toBe(0);
  });

  it("Alt with a mouse button or a pointer press in between (Alt+click)", () => {
    expect(fires([down("Alt"), POINTER, up("Alt")])).toBe(0);
    expect(fires([down("Alt"), POINTER, POINTER, up("Alt")])).toBe(0);
  });

  it("an input method that is composing", () => {
    expect(fires([down("Alt", { composing: true }), up("Alt", { composing: true })])).toBe(0);
    expect(fires([down("Alt"), down("Process", { composing: true }), up("Process", { composing: true }), up("Alt")])).toBe(0);
    // A composition under way when Alt is let go.
    expect(fires([down("Alt"), up("Alt", { composing: true })])).toBe(0);
  });

  it("a window that lost the focus between the press and the release", () => {
    // Alt+Tab: the release goes to the other window, or comes back later with nothing before it.
    expect(fires([down("Alt"), BLUR, up("Alt")])).toBe(0);
    // And the next real press is fine.
    expect(fires([down("Alt"), BLUR, up("Alt"), down("Alt"), up("Alt")])).toBe(1);
    // The key repeating as the focus comes back is not a new press.
    expect(fires([down("Alt"), BLUR, down("Alt", { repeat: true }), up("Alt")])).toBe(0);
  });

  it("a mistaken press does not spoil the next one", () => {
    expect(fires([down("Alt"), down("f"), up("f"), up("Alt"), down("Alt"), up("Alt")])).toBe(1);
    expect(fires([down("Alt"), POINTER, up("Alt"), down("Alt"), up("Alt")])).toBe(1);
  });
});

describe("other keys", () => {
  it("typed while Alt is not down change nothing", () => {
    expect(fires([down("a"), up("a"), down("Control", { ctrl: true }), up("Control"), POINTER, down("Alt"), up("Alt")])).toBe(1);
  });

  it("released while Alt is down do not count: only presses do", () => {
    // `x` was already down when Alt came; it is let go and then Alt is.
    expect(fires([down("x"), down("Alt"), up("x"), up("Alt")])).toBe(1);
  });

  it("a pointer press when Alt is not down changes nothing", () => {
    expect(loneAlt("idle", POINTER)).toEqual({ state: "idle", fire: false });
    expect(loneAlt("tainted", POINTER)).toEqual({ state: "tainted", fire: false });
    expect(loneAlt("armed", POINTER)).toEqual({ state: "tainted", fire: false });
  });

  it("a loss of focus puts every state back to idle without firing", () => {
    for (const state of ["idle", "armed", "tainted"] as const) {
      expect(loneAlt(state, BLUR)).toEqual({ state: "idle", fire: false });
    }
  });

  it("any key, down, taints an armed press and leaves the others as they are", () => {
    expect(loneAlt("armed", down("Enter")).state).toBe("tainted");
    expect(loneAlt("armed", down("Shift", { shift: true })).state).toBe("tainted");
    expect(loneAlt("idle", down("Enter")).state).toBe("idle");
    expect(loneAlt("tainted", down("Enter")).state).toBe("tainted");
    // The release of another key is nothing.
    for (const state of ["idle", "armed", "tainted"] as const) {
      expect(loneAlt(state, up("Enter"))).toEqual({ state, fire: false });
    }
  });

  it("the release of Alt ends every state, and fires only from an armed one", () => {
    expect(loneAlt("idle", up("Alt"))).toEqual({ state: "idle", fire: false });
    expect(loneAlt("tainted", up("Alt"))).toEqual({ state: "idle", fire: false });
    expect(loneAlt("armed", up("Alt"))).toEqual({ state: "idle", fire: true });
  });
});

describe("altKeyInput", () => {
  it("reads the parts of a real keyboard event the detector needs", () => {
    const event = new KeyboardEvent("keydown", { key: "Alt", repeat: true, ctrlKey: true, metaKey: false, shiftKey: true, isComposing: false });
    expect(altKeyInput("keydown", event)).toEqual({
      type: "keydown",
      key: "Alt",
      repeat: true,
      ctrlKey: true,
      metaKey: false,
      shiftKey: true,
      isComposing: false,
    });
    expect(altKeyInput("keyup", new KeyboardEvent("keyup", { key: "Alt" })).type).toBe("keyup");
  });
});

describe("hidesMenuBar", () => {
  it("is true for Escape, whatever is held", () => {
    expect(hidesMenuBar({ key: "Escape", altKey: false })).toBe(true);
    expect(hidesMenuBar({ key: "Escape", altKey: true })).toBe(true);
  });

  it("is true for a key that types or acts, which is how a native Cut, Copy or Paste ends", () => {
    for (const key of ["a", "Enter", "ArrowDown", "Tab", "Backspace", "c", "v", "x", "z", "y", "F5", " "]) {
      expect(hidesMenuBar({ key, altKey: false }), key).toBe(true);
    }
  });

  it("is false for a key with Alt held: the person is reaching for an item with its mnemonic", () => {
    for (const key of ["f", "e", "ArrowLeft", "F4", " "]) {
      expect(hidesMenuBar({ key, altKey: true }), key).toBe(false);
    }
  });

  it("is false for a modifier on its own", () => {
    for (const key of ["Alt", "AltGraph", "Control", "Shift", "Meta", "OS", "CapsLock", "NumLock", "ScrollLock"]) {
      expect(hidesMenuBar({ key, altKey: false }), key).toBe(false);
      expect(hidesMenuBar({ key, altKey: true }), key).toBe(false);
    }
  });
});
