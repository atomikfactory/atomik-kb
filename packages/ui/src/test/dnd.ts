// Helpers to drive HTML5 drag and drop in a test DOM: jsdom has no DataTransfer and no
// layout, so the tests bring a stand-in for the first and set the geometry of rows
// themselves. `fireEvent` returns false when the handler called `preventDefault()`, which
// is how a drop target says that the drop is allowed.

import { fireEvent } from "@testing-library/react";

/** The part of `DataTransfer` the UI uses. */
export class FakeDataTransfer {
  readonly #store = new Map<string, string>();
  effectAllowed = "uninitialized";
  dropEffect = "none";

  setData(type: string, value: string): void {
    this.#store.set(type, value);
  }

  getData(type: string): string {
    return this.#store.get(type) ?? "";
  }

  get types(): string[] {
    return [...this.#store.keys()];
  }

  clearData(): void {
    this.#store.clear();
  }

  setDragImage(): void {
    // The UI does not set a custom drag image.
  }
}

/** Where on a row (or list item) the pointer is. */
export type Zone = "before" | "inside" | "after";

const ROW_TOP = 100;
const ROW_HEIGHT = 28;
const OFFSET: Record<Zone, number> = { before: 2, inside: 14, after: 26 };

/** Gives `element` the geometry of a 28px row, since a test DOM has no layout. */
export function layOut(element: Element): void {
  element.getBoundingClientRect = () => ({
    x: 0,
    y: ROW_TOP,
    left: 0,
    right: 240,
    top: ROW_TOP,
    bottom: ROW_TOP + ROW_HEIGHT,
    width: 240,
    height: ROW_HEIGHT,
    toJSON: () => ({}),
  });
}

/** A drag in progress: `start`, move `over` things (any number of times), then `drop` or `end` it. */
export function beginDrag(source: Element) {
  const dataTransfer = new FakeDataTransfer();
  fireEvent.dragStart(source, { dataTransfer });
  return {
    dataTransfer,
    /** Moves over `target`; returns whether dropping there is allowed. */
    over(target: Element, zone: Zone): boolean {
      layOut(target);
      return !fireEvent.dragOver(target, { dataTransfer, clientY: ROW_TOP + OFFSET[zone] });
    },
    leave(target: Element): void {
      fireEvent.dragLeave(target, { dataTransfer });
    },
    drop(target: Element, zone: Zone): void {
      layOut(target);
      fireEvent.drop(target, { dataTransfer, clientY: ROW_TOP + OFFSET[zone] });
      fireEvent.dragEnd(source, { dataTransfer });
    },
    end(): void {
      fireEvent.dragEnd(source, { dataTransfer });
    },
  };
}

/** Drags `source` onto the `zone` of `target` and drops it there (when that is allowed). */
export function dragAndDrop(source: Element, target: Element, zone: Zone): boolean {
  const drag = beginDrag(source);
  const allowed = drag.over(target, zone);
  if (allowed) {
    drag.drop(target, zone);
  } else {
    drag.end();
  }
  return allowed;
}
