// Where the template picker of `/template` shows (ADR-0056): the editor asks for a choice through `TemplatePickerStore.ask()`
// (the `choose` of its `SubPageTemplates` service) and gets a template or a starter, or `null` when the picker was dismissed.
// Same shape as the "Image from URL" dialog: one picker at a time, asking again gives up the first ask.

import { useSyncExternalStore } from "react";

import type { Anchor } from "../components/FloatingPanel";
import { TemplatePicker } from "../components/TemplatePicker";
import type { TemplateChoice } from "../template-choices";

export class TemplatePickerStore {
  #resolve: ((choice: TemplateChoice | null) => void) | null = null;
  #anchor: Anchor | null = null;
  readonly #listeners = new Set<() => void>();

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  /** Where the picker is open; `null` while it is not. */
  readonly getSnapshot = (): Anchor | null => this.#anchor;

  /** Opens the picker at `anchor` and resolves with the choice, or `null` when it was dismissed or the page closed. */
  readonly ask = (anchor: Anchor): Promise<TemplateChoice | null> => {
    this.finish(null);
    return new Promise((resolve) => {
      this.#resolve = resolve;
      this.#set(anchor);
    });
  };

  /** The picker's answer: a choice, or `null` (Escape, a press outside, the page going away). */
  readonly finish = (choice: TemplateChoice | null): void => {
    const resolve = this.#resolve;
    this.#resolve = null;
    this.#set(null);
    resolve?.(choice);
  };

  #set(anchor: Anchor | null): void {
    if (this.#anchor === anchor) {
      return;
    }
    this.#anchor = anchor;
    for (const listener of [...this.#listeners]) {
      listener();
    }
  }
}

/** Draws the picker of `store` while it is asking. */
export function TemplatePickerHost({ store }: { store: TemplatePickerStore }) {
  const anchor = useSyncExternalStore(store.subscribe, store.getSnapshot);
  if (anchor === null) {
    return null;
  }
  return (
    <TemplatePicker
      anchor={anchor}
      onPick={(choice) => {
        store.finish(choice);
      }}
      onClose={() => {
        store.finish(null);
      }}
    />
  );
}
