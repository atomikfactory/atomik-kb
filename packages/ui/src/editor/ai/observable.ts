// A small observable value for the controllers of the AI features (the writing panel, the recorder,
// the transcription jobs): React reads it with `useObservable` (`useSyncExternalStore`), the controllers
// change it from anywhere, and nothing outside the page editor holds on to it.

import { useSyncExternalStore } from "react";

export class Observable<T> {
  #value: T;
  readonly #listeners = new Set<() => void>();

  constructor(initial: T) {
    this.#value = initial;
  }

  readonly get = (): T => this.#value;

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  /** Replaces the value; listeners run when it is not the same object. */
  set(next: T): void {
    if (Object.is(next, this.#value)) {
      return;
    }
    this.#value = next;
    for (const listener of [...this.#listeners]) {
      listener();
    }
  }

  /** Replaces the value with `update` of it. */
  update(update: (value: T) => T): void {
    this.set(update(this.#value));
  }
}

/** The current value of an observable, re-rendering when it changes. */
export function useObservable<T>(observable: Observable<T>): T {
  return useSyncExternalStore(observable.subscribe, observable.get);
}
