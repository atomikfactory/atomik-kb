// Helpers for the keyboard shortcuts that work anywhere in the app (Ctrl+K, Ctrl+Z, Ctrl+\ and
// the tab keys of ADR-0034).

/** True on the platforms whose shortcut key is Cmd and whose Option+arrows move by word. */
export const IS_MAC = /Mac|iPhone|iPad/.test(navigator.userAgent);

/** The modifier key the platform uses for shortcuts, as it is written in labels. */
export const MODIFIER = IS_MAC ? "Cmd" : "Ctrl";

/** How the keys that go back and forward in a tab are written in labels. */
export const BACK_SHORTCUT = IS_MAC ? "Cmd+[" : "Alt+Left";
export const FORWARD_SHORTCUT = IS_MAC ? "Cmd+]" : "Alt+Right";

/** True for a field whose own keys are not for the shortcuts: an input, a text area or a select. */
export function inFormField(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName);
}

/** True inside an open dialog or menu, which own the keyboard while they are open. */
export function insideOverlay(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    target.closest('[role="dialog"], [role="alertdialog"], [role="menu"]') !== null
  );
}

/** Keys typed into a text field (the page editor included), or inside an open dialog or menu, are not for the shortcuts. */
export function ignoresShortcuts(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  return target.isContentEditable || inFormField(target) || insideOverlay(target);
}

/**
 * For a shortcut that types nothing (Ctrl+\): ignored in a form field and inside a dialog or
 * menu, but not in the page editor, where it is most useful.
 */
export function ignoresNonTypingShortcuts(target: EventTarget | null): boolean {
  return inFormField(target) || insideOverlay(target);
}

/** What a tab key asks for (ADR-0034 section 4). */
export type TabCommand =
  | { kind: "new" }
  | { kind: "close" }
  | { kind: "reopen" }
  /** The next (`1`) or previous (`-1`) tab, wrapping. */
  | { kind: "adjacent"; delta: 1 | -1 }
  /** The tab at a strip position; `-1` is the last one. */
  | { kind: "at"; position: number }
  /** Moves the active tab one place right (`1`) or left (`-1`). */
  | { kind: "move"; delta: 1 | -1 }
  /** Forward (`1`) or back (`-1`) in the active tab's history. */
  | { kind: "history"; delta: 1 | -1 };

/** The parts of a key event that choose a tab command. */
export type TabKeyEvent = Pick<
  KeyboardEvent,
  "key" | "code" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey" | "isComposing"
>;

/**
 * The letter a key stands for, whatever the layout: what it types when that is a Latin letter,
 * else (a Cyrillic or Greek layout, where Ctrl+T still means the physical T key) the physical key.
 */
function letterOf(event: TabKeyEvent): string {
  if (/^[a-z]$/i.test(event.key)) {
    return event.key.toLowerCase();
  }
  const physical = /^Key([A-Z])$/.exec(event.code);
  return physical?.[1]?.toLowerCase() ?? "";
}

/**
 * The digit 1 to 9 a key stands for. The physical key counts as well as what it types, so that
 * the top row of an AZERTY keyboard (`&é"'(-è_ç` without Shift) picks tabs as it does in a browser.
 */
function digitOf(event: TabKeyEvent): number | null {
  const physical = /^Digit([1-9])$/.exec(event.code);
  if (physical?.[1] !== undefined) {
    return Number(physical[1]);
  }
  return /^[1-9]$/.test(event.key) ? Number(event.key) : null;
}

/**
 * The tab command a key event asks for, or `null` when it is not one of the tab keys:
 *
 * - Ctrl/Cmd+T new tab, Ctrl/Cmd+Shift+T reopen the last closed, Ctrl/Cmd+W and Ctrl+F4 close.
 * - Ctrl+Tab and Ctrl+PageDown next, Ctrl+Shift+Tab and Ctrl+PageUp previous, Ctrl+Shift+
 *   PageUp/PageDown move the tab (these use Ctrl itself, also on macOS).
 * - Ctrl/Cmd+1 to 8 that tab, Ctrl/Cmd+9 the last (without Shift, and without Alt, which with Ctrl
 *   is AltGr on many layouts and types a character).
 * - Alt+Left and Alt+Right back and forward, and Ctrl+[ and Ctrl+] (ADR-0040); on macOS, where
 *   Option+arrows move by word, Cmd+[ and Cmd+] instead.
 *
 * `mac` says which of the two history keys applies (the platform's, by default).
 */
export function tabCommand(event: TabKeyEvent, mac: boolean = IS_MAC): TabCommand | null {
  if (event.isComposing) {
    return null;
  }
  const { ctrlKey, metaKey, altKey, shiftKey } = event;

  // Back and forward.
  if (!shiftKey) {
    if (mac) {
      if (metaKey && !ctrlKey && !altKey && (event.key === "[" || event.key === "]")) {
        return { kind: "history", delta: event.key === "[" ? -1 : 1 };
      }
    } else if (altKey && !ctrlKey && !metaKey && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
      return { kind: "history", delta: event.key === "ArrowLeft" ? -1 : 1 };
    } else if (ctrlKey && !altKey && !metaKey && (event.key === "[" || event.key === "]")) {
      // Ctrl+[ and Ctrl+] (ADR-0040) join Alt+Left and Alt+Right. Not with Alt held: AltGr types a
      // bracket on some layouts. The editor binds neither Mod-[ nor Mod-].
      return { kind: "history", delta: event.key === "[" ? -1 : 1 };
    }
  }

  // Keys of Ctrl itself.
  if (ctrlKey && !metaKey && !altKey) {
    switch (event.key) {
      case "Tab":
        return { kind: "adjacent", delta: shiftKey ? -1 : 1 };
      case "PageDown":
        return shiftKey ? { kind: "move", delta: 1 } : { kind: "adjacent", delta: 1 };
      case "PageUp":
        return shiftKey ? { kind: "move", delta: -1 } : { kind: "adjacent", delta: -1 };
      case "F4":
        return shiftKey ? null : { kind: "close" };
      default:
    }
  }

  // Keys of Ctrl or Cmd.
  if ((ctrlKey || metaKey) && !altKey) {
    const letter = letterOf(event);
    if (letter === "t") {
      return shiftKey ? { kind: "reopen" } : { kind: "new" };
    }
    if (letter === "w" && !shiftKey) {
      return { kind: "close" };
    }
    const digit = shiftKey ? null : digitOf(event);
    if (digit !== null) {
      return { kind: "at", position: digit === 9 ? -1 : digit - 1 };
    }
  }
  return null;
}
