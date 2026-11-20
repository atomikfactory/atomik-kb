// The formatting toolbar pinned above the soft keyboard: bold, italic, heading, bullet list, checklist, the `/` menu, the
// microphone, undo and redo. Every button is at least 44 px square and keeps the editor's focus (pointer-down is not
// allowed to move it), so the keyboard stays up while it is used.

import { canRedo, canUndo, redo, t, undo } from "@atomik-kb/ui/mobile";
import type { Editor } from "@tiptap/core";
import { useCallback, useEffect, useReducer, useRef, useState } from "react";
import type { ReactNode } from "react";

interface ToolbarProps {
  editor: Editor | null;
  /** Whether the microphone is listening right now. */
  listening: boolean;
  onDictate: () => void;
  /** Whether the toolbar is shown (the editor or the title has the focus). */
  visible: boolean;
}

function Icon({ children }: { children: ReactNode }) {
  return (
    <svg
      viewBox="0 0 24 24"
      width="22"
      height="22"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {children}
    </svg>
  );
}

interface ButtonProps {
  label: string;
  active?: boolean;
  disabled?: boolean;
  onPress: () => void;
  children: ReactNode;
}

function Button({ label, active = false, disabled = false, onPress, children }: ButtonProps) {
  return (
    <button
      type="button"
      className={active ? "mobile-tool is-active" : "mobile-tool"}
      aria-label={label}
      aria-pressed={active}
      disabled={disabled}
      // Keeps the caret and the keyboard where they are.
      onPointerDown={(event) => {
        event.preventDefault();
      }}
      onClick={onPress}
    >
      {children}
    </button>
  );
}

type Chain = ReturnType<Editor["chain"]>;

export function Toolbar({ editor, listening, onDictate, visible }: ToolbarProps) {
  const [, refresh] = useReducer((n: number) => n + 1, 0);
  useEffect(() => {
    if (editor === null) {
      return undefined;
    }
    editor.on("transaction", refresh);
    return () => {
      editor.off("transaction", refresh);
    };
  }, [editor]);

  // The row scrolls on narrow phones; a fade on the side that has more says so.
  const rowRef = useRef<HTMLDivElement>(null);
  const [more, setMore] = useState({ start: false, end: false });
  const measure = useCallback(() => {
    const row = rowRef.current;
    if (row === null) {
      return;
    }
    const start = row.scrollLeft > 4;
    const end = row.scrollLeft + row.clientWidth < row.scrollWidth - 4;
    setMore((current) => (current.start === start && current.end === end ? current : { start, end }));
  }, []);
  useEffect(() => {
    measure();
    window.addEventListener("resize", measure);
    return () => {
      window.removeEventListener("resize", measure);
    };
  }, [measure, visible]);

  const run = (command: (chain: Chain) => Chain) => () => {
    if (editor !== null) {
      command(editor.chain().focus()).run();
    }
  };
  const on = (name: string) => editor !== null && !editor.isDestroyed && editor.isActive(name);
  const live = editor !== null && !editor.isDestroyed;
  const ready = live && editor.isEditable;

  return (
    <div
      ref={rowRef}
      onScroll={measure}
      className={[
        "mobile-toolbar",
        visible ? "is-visible" : "",
        more.start ? "fade-start" : "",
        more.end ? "fade-end" : "",
      ]
        .filter(Boolean)
        .join(" ")}
      role="toolbar"
      aria-label={t("format.label")}
      aria-hidden={!visible}
    >
      <Button label={t("format.bold")} active={on("bold")} disabled={!ready} onPress={run((c) => c.toggleBold())}>
        <Icon>
          <path d="M7 5h6a3.5 3.5 0 0 1 0 7H7zM7 12h7a3.5 3.5 0 0 1 0 7H7z" />
        </Icon>
      </Button>
      <Button label={t("format.italic")} active={on("italic")} disabled={!ready} onPress={run((c) => c.toggleItalic())}>
        <Icon>
          <path d="M19 4h-9M14 20H5M15 4 9 20" />
        </Icon>
      </Button>
      <Button
        label={t("slash.heading2")}
        active={on("heading")}
        disabled={!ready}
        onPress={run((c) => c.toggleHeading({ level: 2 }))}
      >
        <Icon>
          <path d="M5 5v14M19 5v14M5 12h14" />
        </Icon>
      </Button>
      <Button label={t("slash.bulletList")} active={on("bulletList")} disabled={!ready} onPress={run((c) => c.toggleBulletList())}>
        <Icon>
          <path d="M9 6h11M9 12h11M9 18h11M4 6h.01M4 12h.01M4 18h.01" />
        </Icon>
      </Button>
      <Button label={t("slash.taskList")} active={on("taskList")} disabled={!ready} onPress={run((c) => c.toggleTaskList())}>
        <Icon>
          <path d="m3.5 6 1.5 1.5L8 4.5M3.5 17l1.5 1.5L8 15.5M12 6h9M12 17h9" />
        </Icon>
      </Button>
      <Button label={t("slash.label")} disabled={!ready} onPress={run((c) => c.insertContent("/"))}>
        <Icon>
          <path d="M16 4 8 20" />
        </Icon>
      </Button>
      <Button label={t("slash.microphone")} active={listening} disabled={!ready} onPress={onDictate}>
        <Icon>
          <rect x="9" y="3" width="6" height="11" rx="3" />
          <path d="M5 11a7 7 0 0 0 14 0M12 18v3" />
        </Icon>
      </Button>
      <Button
        label={t("sidebar.undo")}
        disabled={!ready || !canUndo(editor.state)}
        onPress={() => {
          if (live) {
            undo(editor.state, editor.view.dispatch);
            editor.view.focus();
          }
        }}
      >
        <Icon>
          <path d="M9 14 4 9l5-5M4 9h10a6 6 0 0 1 0 12h-3" />
        </Icon>
      </Button>
      <Button
        label={t("sidebar.redo")}
        disabled={!ready || !canRedo(editor.state)}
        onPress={() => {
          if (live) {
            redo(editor.state, editor.view.dispatch);
            editor.view.focus();
          }
        }}
      >
        <Icon>
          <path d="m15 14 5-5-5-5M20 9H10a6 6 0 0 0 0 12h3" />
        </Icon>
      </Button>
    </div>
  );
}
