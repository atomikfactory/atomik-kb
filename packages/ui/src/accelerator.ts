// The quick-capture shortcut as the Settings screen records it (ADR-0040): a key press turned into the accelerator
// text that the device setting `app.shortcut.capture` holds and the desktop shell registers (`Ctrl+Alt+N`).
//
// The text is the shell's canonical form: modifiers in the order `Ctrl`, `Alt`, `Shift`, `Super`, then one key named
// as the shell names it (a letter, a digit, `F1` to `F24`, `Space`, `Enter`, `Tab`, `Backspace`, `Delete`,
// `Insert`, `Home`, `End`, `PageUp`, `PageDown`, `ArrowUp` and its three siblings, `Escape` and the punctuation
// keys of the main block). The shell reads it strictly again (`src-tauri/src/accelerator.rs`) and refuses what it
// would not write: a combination needs at least one of Ctrl, Alt or Super (Shift alone would take the capital
// letters away from every other program). The key is read from the physical key (`KeyboardEvent.code`), not the
// character, so Shift and the keyboard layout do not change what is recorded.

/** The parts of a key event that make an accelerator. */
export interface KeyPress {
  /** The physical key (`KeyboardEvent.code`): `KeyN`, `Digit1`, `F5`, `Space`, `ArrowUp`, `ControlLeft`... */
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

/** What a key press means to the recorder. */
export type KeyCapture =
  /** A complete combination: record it. */
  | { kind: "accelerator"; accelerator: string }
  /** Only modifiers are down so far: wait for the key. */
  | { kind: "pending" }
  /** A key without Ctrl, Alt or Super: not enough for a shortcut that works in every program. */
  | { kind: "noModifier" }
  /** A key the shell does not accept (the numeric keypad, media keys, ...). */
  | { kind: "unsupported" };

/** The shortcut used while the setting is unset (the shell's default). */
export const DEFAULT_CAPTURE_SHORTCUT = "Ctrl+Alt+N";

const MODIFIER_CODES: ReadonlySet<string> = new Set([
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "ShiftLeft",
  "ShiftRight",
  "MetaLeft",
  "MetaRight",
  "OS",
]);

/** The keys whose `code` is already the name the shell uses. */
const NAMED_KEYS: ReadonlySet<string> = new Set([
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
]);

/** The shell's name for the physical key `code`, or `null` when the shell has no such key. */
export function keyName(code: string): string | null {
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter !== null) {
    return letter[1] ?? null;
  }
  const digit = /^Digit([0-9])$/.exec(code);
  if (digit !== null) {
    return digit[1] ?? null;
  }
  const functionKey = /^F([1-9]|1[0-9]|2[0-4])$/.exec(code);
  if (functionKey !== null) {
    return code;
  }
  return NAMED_KEYS.has(code) ? code : null;
}

/** Turns a key press into what the recorder does with it (see {@link KeyCapture}). */
export function captureKey(press: KeyPress): KeyCapture {
  if (MODIFIER_CODES.has(press.code)) {
    return { kind: "pending" };
  }
  const key = keyName(press.code);
  if (key === null) {
    return { kind: "unsupported" };
  }
  if (!(press.ctrlKey || press.altKey || press.metaKey)) {
    return { kind: "noModifier" };
  }
  const parts: string[] = [];
  if (press.ctrlKey) {
    parts.push("Ctrl");
  }
  if (press.altKey) {
    parts.push("Alt");
  }
  if (press.shiftKey) {
    parts.push("Shift");
  }
  if (press.metaKey) {
    parts.push("Super");
  }
  parts.push(key);
  return { kind: "accelerator", accelerator: parts.join("+") };
}
