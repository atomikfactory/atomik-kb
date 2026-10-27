// The popup of a Tiptap suggestion (the `/` menu, the `[[` page picker). The suggestion
// plugin drives a small store through its `render()` hooks; a React component draws the
// store's state next to the cursor. Up and Down move (and wrap), PageUp and PageDown jump,
// Enter or Tab choose, Escape closes. Items may belong to sections, which get a header each
// while nothing is typed.

import type { SuggestionKeyDownProps, SuggestionOptions, SuggestionProps } from "@tiptap/suggestion";
import { useEffect, useId, useRef, useSyncExternalStore } from "react";
import type { CSSProperties, ReactNode } from "react";
import { createPortal } from "react-dom";

export interface SuggestionMenuState<T> {
  items: readonly T[];
  /** Index of the highlighted item. */
  selected: number;
  query: string;
  /** Where the typed trigger is, in viewport coordinates. */
  rect: { left: number; top: number; bottom: number } | null;
}

type Renderer<T> = ReturnType<NonNullable<SuggestionOptions<T, T>["render"]>>;

/** How far PageUp and PageDown move the highlight. */
const PAGE_STEP = 6;

export class SuggestionMenuStore<T> {
  #state: SuggestionMenuState<T> | null = null;
  #command: ((item: T) => void) | null = null;
  readonly #listeners = new Set<() => void>();

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  readonly getSnapshot = (): SuggestionMenuState<T> | null => this.#state;

  /** The `render` option of a suggestion, wired to this store. */
  renderer(): Renderer<T> {
    return {
      onStart: (props: SuggestionProps<T, T>) => {
        this.#show(props);
      },
      onUpdate: (props: SuggestionProps<T, T>) => {
        this.#show(props);
      },
      onExit: () => {
        this.#command = null;
        this.#set(null);
      },
      onKeyDown: ({ event }: SuggestionKeyDownProps) => this.#keyDown(event),
    };
  }

  /** Highlights an item (pointer hover). */
  select(index: number): void {
    if (this.#state !== null && index !== this.#state.selected && index >= 0 && index < this.#state.items.length) {
      this.#set({ ...this.#state, selected: index });
    }
  }

  /** Runs the suggestion's command with an item. */
  choose(index: number): void {
    const item = this.#state?.items[index];
    if (item !== undefined && this.#command !== null) {
      this.#command(item);
    }
  }

  #show(props: SuggestionProps<T, T>): void {
    this.#command = props.command;
    const rect = props.clientRect?.() ?? null;
    const previous = this.#state;
    const selected =
      previous !== null && previous.query === props.query ? Math.min(previous.selected, props.items.length - 1) : 0;
    this.#set({
      items: props.items,
      selected: Math.max(0, selected),
      query: props.query,
      rect: rect === null ? null : { left: rect.left, top: rect.top, bottom: rect.bottom },
    });
  }

  #keyDown(event: KeyboardEvent): boolean {
    const state = this.#state;
    if (state === null) {
      return false;
    }
    const count = state.items.length;
    switch (event.key) {
      case "ArrowDown":
        if (count > 0) {
          this.#set({ ...state, selected: (state.selected + 1) % count });
        }
        return true;
      case "ArrowUp":
        if (count > 0) {
          this.#set({ ...state, selected: (state.selected - 1 + count) % count });
        }
        return true;
      case "PageDown":
        if (count > 0) {
          this.#set({ ...state, selected: Math.min(state.selected + PAGE_STEP, count - 1) });
        }
        return true;
      case "PageUp":
        if (count > 0) {
          this.#set({ ...state, selected: Math.max(state.selected - PAGE_STEP, 0) });
        }
        return true;
      case "Enter":
      case "Tab":
        if (count === 0) {
          return false;
        }
        this.choose(state.selected);
        return true;
      default:
        return false; // Escape is the suggestion plugin's own
    }
  }

  #set(state: SuggestionMenuState<T> | null): void {
    this.#state = state;
    for (const listener of [...this.#listeners]) {
      listener();
    }
  }
}

/** How a menu groups its items: the section each belongs to and the header of a section. */
export interface SuggestionSections<T> {
  /** The section of an item; `undefined` for an item that is listed without a header. */
  of: (item: T) => string | undefined;
  label: (section: string) => string;
}

export interface SuggestionMenuProps<T> {
  store: SuggestionMenuStore<T>;
  /** Accessible name of the list. */
  label: string;
  /** Shown when nothing matches; `""` shows nothing then (the menu stays hidden until there is an item). */
  emptyText: string;
  itemKey: (item: T) => string;
  renderItem: (item: T) => ReactNode;
  /** Items shown but not available (the suggestion's command ignores them). */
  isDisabled?: (item: T) => boolean;
  /**
   * Section headers between the groups of consecutive items of one section. Shown only while
   * nothing is typed: matches are listed best first, not by section. A menu without this, or
   * whose items have no section, draws no header.
   */
  sections?: SuggestionSections<T>;
  /** Class names added to the popup. */
  className?: string;
}

/** Usual height of the menu, and the room below the cursor it needs before it opens above. */
const MENU_HEIGHT = 340;
const MENU_WIDTH = 300; // the widest menu (the `/` menu): a menu never leaves the window
const MIN_ROOM = 220;
const GAP = 6;

/** Below the cursor, or above it when the window has no room below. */
function placement(rect: SuggestionMenuState<unknown>["rect"]): CSSProperties {
  const left = Math.max(8, Math.min(rect?.left ?? 0, window.innerWidth - MENU_WIDTH - 8));
  const top = rect?.top ?? 0;
  const bottom = rect?.bottom ?? 0;
  const below = window.innerHeight - bottom - GAP - 8;
  if (below < Math.min(MIN_ROOM, MENU_HEIGHT) && top > below) {
    return { left, bottom: window.innerHeight - top + GAP, maxHeight: Math.min(MENU_HEIGHT, top - GAP - 8) };
  }
  return { left, top: bottom + GAP, maxHeight: Math.min(MENU_HEIGHT, Math.max(below, 120)) };
}

/** Consecutive items of one section (`section` is `undefined` for items without one). */
interface Run<T> {
  section: string | undefined;
  entries: { item: T; index: number }[];
}

function runsOf<T>(items: readonly T[], sectionOf: ((item: T) => string | undefined) | undefined): Run<T>[] {
  const runs: Run<T>[] = [];
  items.forEach((item, index) => {
    const section = sectionOf?.(item);
    const last = runs.at(-1);
    if (last !== undefined && last.section === section) {
      last.entries.push({ item, index });
    } else {
      runs.push({ section, entries: [{ item, index }] });
    }
  });
  return runs;
}

/** Draws a suggestion store's popup next to the cursor, in the document body. */
export function SuggestionMenu<T>({
  store,
  label,
  emptyText,
  itemKey,
  renderItem,
  isDisabled,
  sections,
  className,
}: SuggestionMenuProps<T>) {
  const state = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const id = useId();
  const menu = useRef<HTMLDivElement>(null);
  const selected = state?.selected;
  const optionId = (index: number): string => `${id}-option-${String(index)}`;
  // Keep the highlighted option in view, with the header of its section when it opens one.
  useEffect(() => {
    if (selected === undefined) {
      return;
    }
    const option = document.getElementById(`${id}-option-${String(selected)}`);
    if (option === null) {
      return;
    }
    option.scrollIntoView({ block: "nearest" });
    const header = option.previousElementSibling;
    const popup = menu.current;
    if (popup !== null && header instanceof HTMLElement && header.classList.contains("suggestion-section")) {
      const top = Math.max(0, header.offsetTop - 4);
      if (top < popup.scrollTop) {
        popup.scrollTop = top;
      }
    }
  }, [id, selected]);
  if (state === null || (emptyText === "" && state.items.length === 0)) {
    return null;
  }
  const sectionOf = sections !== undefined && state.query.trim() === "" ? sections.of : undefined;
  const renderOption = ({ item, index }: { item: T; index: number }) => {
    const disabled = isDisabled?.(item) === true;
    return (
      <div
        key={itemKey(item)}
        id={optionId(index)}
        role="option"
        aria-selected={index === state.selected}
        aria-disabled={disabled ? true : undefined}
        className={`suggestion-item${index === state.selected ? " is-selected" : ""}${disabled ? " is-disabled" : ""}`}
        // Not `mouseenter`: a list that scrolls under a resting pointer would take the highlight.
        onMouseMove={() => {
          store.select(index);
        }}
        onClick={() => {
          store.choose(index);
        }}
      >
        {renderItem(item)}
      </div>
    );
  };
  return createPortal(
    <div
      ref={menu}
      className={`floating suggestion-menu${className === undefined ? "" : ` ${className}`}`}
      style={placement(state.rect)}
      // Keep the editor focused and its selection where it is.
      onMouseDown={(event) => {
        event.preventDefault();
      }}
    >
      {state.items.length === 0 ? (
        <p className="suggestion-empty" role="status">
          {emptyText}
        </p>
      ) : (
        <div
          role="listbox"
          aria-label={label}
          aria-activedescendant={optionId(state.selected)}
          id={id}
          className="suggestion-list"
        >
          {runsOf(state.items, sectionOf).map((run) => {
            const first = run.entries[0];
            if (run.section === undefined || first === undefined) {
              return run.entries.map(renderOption);
            }
            const headerId = `${id}-section-${String(first.index)}`;
            return (
              <div key={headerId} role="group" aria-labelledby={headerId} className="suggestion-group">
                <div id={headerId} className="suggestion-section">
                  {sections?.label(run.section)}
                </div>
                {run.entries.map(renderOption)}
              </div>
            );
          })}
        </div>
      )}
    </div>,
    document.body,
  );
}
