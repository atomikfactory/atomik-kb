// The desktop shell's native menu bar (ADR-0040 item 12) as the page sees it: the window has File, Edit, View,
// Window and Help, hidden until a lone Alt press. The shell cannot see keys typed into the webview, so the page
// detects the press and asks for the bar (`menuBarShow`); the shell hides it again after a choice, when the window
// loses focus and when the page reloads, and tells the page each change (`menuBar` messages, `menuBarVisible`).
// The page also hides it (`menuBarHide`) on Escape and on the first key or click in the page while it is shown,
// which is also how a choice of one of the native Edit items ends (muda sends the key to the page). A lone Alt
// while the bar is shown asks for nothing: it is the operating system's, which starts the menu's keyboard mode.
//
// Nothing here is a menu: the items, their words and what they do are the shell's and `store-shell.ts`'s.
//
// A **lone Alt press** is Alt down and up with no other key, no mouse button and no composition between, and not AltGr
// (which is Ctrl+Alt on Windows). [`loneAlt`] is the pure state machine; [`useMenuBarKeys`] feeds it the page's events.

import { useEffect } from "react";

import { IS_MAC } from "./shortcuts";
import { useStoreApi } from "./store-context";

/**
 * Where a press of Alt is:
 * - `idle`: Alt is not down (or the page does not know it is);
 * - `armed`: Alt is down and nothing else happened since: its release would be a lone press;
 * - `tainted`: Alt is down but something else happened (a key, a click, another Alt): its release is nothing.
 */
export type AltState = "idle" | "armed" | "tainted";

/** The parts of a key event the detector reads. */
export interface AltKeyInput {
  type: "keydown" | "keyup";
  key: string;
  /** The key was repeated by the keyboard (held down). */
  repeat: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  /** An input method is composing (`key` is then often "Process"). */
  isComposing: boolean;
}

/** What the detector hears: keys, a mouse button or pointer press (`pointer`), and the window losing focus (`blur`). */
export type AltInput = AltKeyInput | { type: "pointer" } | { type: "blur" };

export interface AltStep {
  state: AltState;
  /** The release of a lone Alt press: the menu bar is asked for. */
  fire: boolean;
}

/** `input` taken from a real keyboard event. */
export function altKeyInput(type: "keydown" | "keyup", event: KeyboardEvent): AltKeyInput {
  return {
    type,
    key: event.key,
    repeat: event.repeat,
    ctrlKey: event.ctrlKey,
    metaKey: event.metaKey,
    shiftKey: event.shiftKey,
    isComposing: event.isComposing,
  };
}

/**
 * The next state after `input`, and whether it completed a lone Alt press.
 *
 * - Alt down, with Ctrl, Meta or Shift held or a composition under way, is not a lone press (`tainted`); Alt down
 *   while another Alt is down too; an auto-repeat of Alt changes nothing.
 * - Any other key down, a mouse or pointer press and a loss of focus end the chance of the press being a lone one
 *   (focus lost: the state starts again from `idle`, since the release will not come to this page).
 * - Alt up fires only from `armed`, and always returns to `idle`.
 * - AltGr is `AltGraph` for the page, never `Alt`, and arrives with a Ctrl; it never arms.
 */
export function loneAlt(state: AltState, input: AltInput): AltStep {
  if (input.type === "blur") {
    return { state: "idle", fire: false };
  }
  if (input.type === "pointer") {
    return { state: state === "armed" ? "tainted" : state, fire: false };
  }
  const isAlt = input.key === "Alt";
  if (input.type === "keydown") {
    if (isAlt) {
      if (input.repeat) {
        return { state, fire: false };
      }
      const chord = input.ctrlKey || input.metaKey || input.shiftKey || input.isComposing;
      return { state: state === "idle" && !chord ? "armed" : "tainted", fire: false };
    }
    return { state: state === "armed" ? "tainted" : state, fire: false };
  }
  // keyup
  if (!isAlt) {
    return { state, fire: false };
  }
  const chord = input.ctrlKey || input.metaKey || input.shiftKey || input.isComposing;
  return { state: "idle", fire: state === "armed" && !chord };
}

/** Keys that are modifiers on their own: pressing one is not typing, so it does not dismiss the bar. */
const MODIFIER_KEYS: ReadonlySet<string> = new Set(["Alt", "AltGraph", "Control", "Shift", "Meta", "OS", "CapsLock", "NumLock", "ScrollLock"]);

/**
 * Whether a key pressed in the page, while the menu bar is shown, sends the bar away: Escape, and any key that types
 * or acts (with Ctrl, for instance, which is what a native Cut, Copy or Paste sends). Not a modifier alone, and not a
 * key with Alt held, which is the person reaching for an item with its mnemonic (Alt+F for File).
 */
export function hidesMenuBar(event: Pick<KeyboardEvent, "key" | "altKey">): boolean {
  if (event.key === "Escape") {
    return true;
  }
  return !event.altKey && !MODIFIER_KEYS.has(event.key);
}

/**
 * Listens for the lone Alt press (and, while the bar is shown, for what sends it away) and asks the shell. Mount it
 * once, in the main layout. It does nothing where the shell has no menu bar (the web build, a shell without one) and
 * on macOS, where Option alone is not a menu key and the menu is the system's.
 */
export function useMenuBarKeys(): void {
  const store = useStoreApi();
  useEffect(() => {
    if (IS_MAC || !store.getState().menuBar) {
      return;
    }
    let state: AltState = "idle";
    const feed = (input: AltInput): void => {
      const step = loneAlt(state, input);
      state = step.state;
      if (step.fire && !store.getState().menuBarVisible) {
        store.getState().menuBarShow();
      }
    };
    const hideIfShown = (): void => {
      if (store.getState().menuBarVisible) {
        store.getState().menuBarHide();
      }
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      feed(altKeyInput("keydown", event));
      if (hidesMenuBar(event)) {
        hideIfShown();
      }
    };
    const onKeyUp = (event: KeyboardEvent): void => {
      feed(altKeyInput("keyup", event));
    };
    const onPointer = (): void => {
      feed({ type: "pointer" });
      hideIfShown();
    };
    const onBlur = (): void => {
      feed({ type: "blur" });
    };
    // In the capture phase, so that a handler that stops the event (a dialog, the editor) cannot hide it from here.
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    window.addEventListener("pointerdown", onPointer, true);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
      window.removeEventListener("pointerdown", onPointer, true);
      window.removeEventListener("blur", onBlur);
      // The layout goes (the app locked): the bar must not stay behind it.
      hideIfShown();
    };
  }, [store]);
}
