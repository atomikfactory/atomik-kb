// A transport with a desktop shell around the in-memory core, for the tests of what the shell tells the page and what
// the page tells the shell (ADR-0040): the in-memory core has no shell, so it has neither `onShellEvent` nor
// `shellLabelsSet`; this wraps it with both (and the menu bar's `menuBarShow` and `menuBarHide`, recording the calls),
// driven by the test, the way the real shell behaves: what came before the first listener (a link, a capture) is
// delivered at the subscription, and so is the state of the shortcut.

import type { MemoryTransport, ShellEvent, ShellLabels, ShortcutState, Transport } from "@atomik-kb/core-client";

export interface ShellTransport {
  /** The in-memory transport with the shell's members; give it to `renderWith`. */
  transport: Transport;
  /** Sends a message to every listener, as the shell does. */
  emit: (event: ShellEvent) => void;
  /** How many listeners are registered now. */
  listeners: () => number;
  /** Every `shellLabelsSet` message the page sent, in order. */
  labels: ShellLabels[];
  /** Makes the next `shellLabelsSet` reject (the shell refused a label). */
  failLabels: (error: Error) => void;
  /** Every call of the menu bar's commands the page made, in order. */
  menuBar: ("show" | "hide")[];
  /** Makes the next menu bar command reject (the shell has no menu bar). */
  failMenuBar: (error: Error) => void;
}

export interface ShellOptions {
  /** Optional transport methods the shell has besides its own (the dialogs of import and export). */
  methods?: Pick<Transport, "exportRun" | "importRun">;
  /** The shell has no menu bar (an older one): `menuBarShow` and `menuBarHide` are absent. Default: it has. */
  noMenuBar?: boolean;
  /** Messages that came before the first listener: delivered, in order, at the first subscription. */
  held?: readonly ShellEvent[];
  /** The state of the capture shortcut the shell reports at every subscription. */
  shortcut?: ShortcutState;
}

/** `memory` with the shell's `onShellEvent` and `shellLabelsSet`. */
export function withShell(memory: MemoryTransport, options: ShellOptions = {}): ShellTransport {
  const listeners = new Set<(event: ShellEvent) => void>();
  const labels: ShellLabels[] = [];
  let pending = [...(options.held ?? [])];
  let failure: Error | undefined;
  const menuBar: ("show" | "hide")[] = [];
  let menuBarFailure: Error | undefined;
  const menuBarCall = (call: "show" | "hide"): Promise<void> => {
    menuBar.push(call);
    if (menuBarFailure !== undefined) {
      const error = menuBarFailure;
      menuBarFailure = undefined;
      return Promise.reject(error);
    }
    return Promise.resolve();
  };
  const onShellEvent = (listener: (event: ShellEvent) => void): (() => void) => {
    listeners.add(listener);
    if (options.shortcut !== undefined) {
      listener({ type: "shortcut", data: { state: options.shortcut } });
    }
    const held = pending;
    pending = [];
    for (const event of held) {
      listener(event);
    }
    return () => {
      listeners.delete(listener);
    };
  };
  const shellLabelsSet = (update: ShellLabels): Promise<void> => {
    labels.push(update);
    if (failure !== undefined) {
      const error = failure;
      failure = undefined;
      return Promise.reject(error);
    }
    return Promise.resolve();
  };
  // Everything else is the in-memory transport, called on itself (it has private fields).
  const transport = new Proxy<Transport>(memory, {
    get(target, property) {
      if (property === "onShellEvent") {
        return onShellEvent;
      }
      if (property === "shellLabelsSet") {
        return shellLabelsSet;
      }
      if (property === "menuBarShow" || property === "menuBarHide") {
        if (options.noMenuBar === true) {
          return undefined;
        }
        return () => menuBarCall(property === "menuBarShow" ? "show" : "hide");
      }
      if (property === "exportRun" || property === "importRun") {
        return options.methods?.[property];
      }
      const value: unknown = Reflect.get(target, property, target);
      return typeof value === "function" ? (value as (...args: unknown[]) => unknown).bind(target) : value;
    },
  });
  return {
    transport,
    emit: (event) => {
      for (const listener of [...listeners]) {
        listener(event);
      }
    },
    listeners: () => listeners.size,
    labels,
    failLabels: (error) => {
      failure = error;
    },
    menuBar,
    failMenuBar: (error) => {
      menuBarFailure = error;
    },
  };
}
