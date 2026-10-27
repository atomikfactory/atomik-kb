// A transport with the links of the operating system, for the tests of deep links (ADR-0040): the in-memory
// core has no shell around it, so it has no `onShellEvent`; this wraps it with the shell's (`test/shell.ts`) and
// drives only the links, the way the desktop shell does: a link that came before the first listener is delivered at
// the subscription.

import type { DeepLink, MemoryTransport, Transport } from "@atomik-kb/core-client";

import { withShell } from "./shell";

export interface LinkedTransport {
  /** The in-memory transport with the shell's members; give it to `renderWith`. */
  transport: Transport;
  /** Sends a link to every listener, as the shell does when the OS opens one. */
  emit: (link: DeepLink) => void;
  /** How many listeners are registered now. */
  listeners: () => number;
}

/** `memory` with a shell; `held` is the link the app was started with (delivered at the first subscription). */
export function withDeepLinks(memory: MemoryTransport, held?: DeepLink): LinkedTransport {
  const shell = withShell(memory, { held: held === undefined ? [] : [{ type: "deepLink", data: held }] });
  return {
    transport: shell.transport,
    emit: (link) => {
      shell.emit({ type: "deepLink", data: link });
    },
    listeners: shell.listeners,
  };
}
