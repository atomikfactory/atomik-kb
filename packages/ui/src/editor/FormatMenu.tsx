// The bubble menu above selected text: bold, italic, underline, strike, code, text colour and
// highlight (a popover of swatches), link and, for a member who can comment (ADR-0048), "Comment", which starts a comment
// anchored to the selection. A Commenter's editor is read-only: the menu then offers "Comment" alone.

import type { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { useEditorState } from "@tiptap/react";
import { useId, useState } from "react";
import type { FormEvent, ReactNode } from "react";
import { createPortal } from "react-dom";

import type { CommentAnchor } from "@atomik-kb/core-client";

import { anchorBelow } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { isExternalHref } from "../store-content";
import { anchorOfSelection } from "./comment-anchors";
import { useAiCapabilities } from "./ai/availability";
import { useEditorAi } from "./ai/EditorAi";
import { isPaletteColor } from "./palette";
import type { PaletteColor } from "./palette";
import { selectionColor } from "./text-color";
import { TextColorPicker } from "./TextColorPicker";

type MarkName = "bold" | "italic" | "underline" | "strike" | "code";

const MARKS: readonly { name: MarkName; label: MessageKey; text: ReactNode; className: string }[] = [
  { name: "bold", label: "format.bold", text: "B", className: "format-bold" },
  { name: "italic", label: "format.italic", text: "I", className: "format-italic" },
  { name: "underline", label: "format.underline", text: "U", className: "format-underline" },
  { name: "strike", label: "format.strike", text: "S", className: "format-strike" },
  { name: "code", label: "format.code", text: "</>", className: "format-code" },
];

interface MenuState {
  visible: boolean;
  /** The editor takes edits (a Commenter's does not): the menu then holds "Comment" alone. */
  editable: boolean;
  from: number;
  active: Record<MarkName | "link", boolean>;
  href: string;
  /** The text colour of the selection: a name, `null` for none, `undefined` when mixed. */
  textColor: string | null | undefined;
  /** The highlight of the selection, as `textColor`. */
  highlight: string | null | undefined;
}

function readState(editor: Editor, keepOpen: boolean, canComment: boolean): MenuState {
  const { selection } = editor.state;
  // A code or math block holds plain text (no marks): there is nothing to format in it.
  const textSelected =
    selection instanceof TextSelection && !selection.empty && selection.$from.parent.type.spec.code !== true;
  const active = {
    bold: editor.isActive("bold"),
    italic: editor.isActive("italic"),
    underline: editor.isActive("underline"),
    strike: editor.isActive("strike"),
    code: editor.isActive("code"),
    link: editor.isActive("link"),
  };
  const href: unknown = editor.getAttributes("link").href;
  return {
    // An editable editor shows it while it has the focus (or a popover of the menu is open); a read-only one has no focus to
    // follow, so it shows it for a selected range, to a member who can comment.
    editable: editor.isEditable,
    visible: textSelected && (editor.isEditable ? editor.isFocused || keepOpen : canComment),
    from: selection.from,
    active,
    href: typeof href === "string" ? href : "",
    // Only read while there is text selected (the menu is not shown otherwise).
    textColor: textSelected ? selectionColor(editor, "textColor") : null,
    highlight: textSelected ? selectionColor(editor, "highlight") : null,
  };
}

/** `https://` in front of an address typed without a scheme. */
export function normalizeHref(input: string): string {
  const trimmed = input.trim();
  if (trimmed === "" || /^[a-z][a-z0-9+.-]*:/i.test(trimmed)) {
    return trimmed;
  }
  return `https://${trimmed}`;
}

/** Where the selection starts on screen (the menu sits above it). */
function anchorOf(editor: Editor, pos: number): { left: number; top: number } {
  try {
    const coords = editor.view.coordsAtPos(pos);
    return { left: Math.max(8, coords.left), top: Math.max(48, coords.top - 8) };
  } catch {
    return { left: 8, top: 48 }; // no layout (tests), or a position that just went away
  }
}

export interface FormatMenuProps {
  editor: Editor;
  /** This account may comment (Commenter and up): "Comment" is offered. */
  canComment?: boolean;
  /** Starts a comment anchored to the selection. */
  onComment?: (anchor: CommentAnchor) => void;
}

export function FormatMenu({ editor, canComment = false, onComment }: FormatMenuProps) {
  const [linkOpen, setLinkOpen] = useState(false);
  const [hasFocus, setHasFocus] = useState(false);
  // The colour popover: where it opens and the button that opened it (null: closed).
  const [palette, setPalette] = useState<{ anchor: Anchor; opener: HTMLElement } | null>(null);
  const state = useEditorState({
    editor,
    selector: ({ editor: current }) => readState(current, hasFocus || linkOpen || palette !== null, canComment),
  });
  const ai = useEditorAi();
  const askAi = useAiCapabilities().writing && ai !== null;
  const editable = state.editable;

  if (palette !== null && !state.visible) {
    setPalette(null); // the selection went away under the open popover
  }
  if (!(state.visible || linkOpen)) {
    return null;
  }
  const position = anchorOf(editor, state.from);

  /** Colours the selection (or what is typed next), and puts focus back in the text. */
  function pickTextColor(color: PaletteColor | null): void {
    const chain = editor.chain().focus();
    (color === null ? chain.unsetTextColor() : chain.setTextColor(color)).run();
  }

  function pickHighlight(color: PaletteColor | null): void {
    const chain = editor.chain().focus();
    (color === null ? chain.unsetHighlight() : chain.setHighlight(color)).run();
  }

  /** Starts a comment on the selected words and lets the selection go, so that the menu does too. */
  function comment(): void {
    const anchor = anchorOfSelection(editor.state);
    if (anchor === null || onComment === undefined) {
      return;
    }
    onComment(anchor);
    editor.commands.setTextSelection(editor.state.selection.to);
  }

  // The controls that write the text: not for a read-only editor (a Commenter's).
  const formatControls = (
    <>
      {MARKS.map((mark) => (
        <button
          key={mark.name}
          type="button"
          className={`format-button ${mark.className}`}
          aria-label={t(mark.label)}
          title={t(mark.label)}
          aria-pressed={state.active[mark.name]}
          onClick={() => {
            editor.chain().focus().toggleMark(mark.name).run();
          }}
        >
          {mark.text}
        </button>
      ))}
      <button
        type="button"
        className="format-button format-color"
        aria-label={t("format.color")}
        title={t("format.color")}
        aria-haspopup="dialog"
        aria-expanded={palette !== null}
        onClick={(event) => {
          const opener = event.currentTarget;
          setPalette((current) => (current === null ? { anchor: anchorBelow(opener), opener } : null));
        }}
      >
        <span
          className="format-color-glyph"
          data-text={isPaletteColor(state.textColor) ? state.textColor : undefined}
          data-fill={isPaletteColor(state.highlight) ? state.highlight : undefined}
          aria-hidden="true"
        >
          A
        </span>
      </button>
      <button
        type="button"
        className="format-button format-link"
        aria-label={t("format.link")}
        title={t("format.link")}
        aria-pressed={state.active.link}
        onClick={() => {
          setLinkOpen(true);
        }}
      >
        {t("format.linkShort")}
      </button>
      {askAi && (
        <button
          type="button"
          className="format-button format-ai"
          aria-label={t("aiWriter.buttonLabel")}
          title={t("aiWriter.buttonLabel")}
          aria-haspopup="dialog"
          onClick={() => {
            ai.writer.open({ kind: "selection" });
          }}
        >
          {t("aiWriter.button")}
        </button>
      )}
    </>
  );

  const toolbar = createPortal(
    <div
      role="toolbar"
      aria-label={t("format.label")}
      className="floating format-menu"
      style={position}
      onFocus={() => {
        setHasFocus(true);
      }}
      onBlur={(event) => {
        if (!(event.relatedTarget instanceof Node && event.currentTarget.contains(event.relatedTarget))) {
          setHasFocus(false);
        }
      }}
      onMouseDown={(event) => {
        if (!(event.target instanceof HTMLInputElement)) {
          event.preventDefault(); // keep the editor's selection
        }
      }}
    >
      {linkOpen ? (
        <LinkForm
          initial={state.href}
          canRemove={state.active.link}
          onApply={(href) => {
            editor.chain().focus().extendMarkRange("link").setLink({ href }).run();
            setLinkOpen(false);
          }}
          onRemove={() => {
            editor.chain().focus().extendMarkRange("link").unsetLink().run();
            setLinkOpen(false);
          }}
          onCancel={() => {
            setLinkOpen(false);
            editor.commands.focus();
          }}
        />
      ) : (
        <>
          {editable && formatControls}
          {canComment && onComment !== undefined && (
            <button
              type="button"
              className="format-button format-comment"
              aria-label={t("comments.add")}
              title={t("comments.add")}
              onClick={comment}
            >
              {t("comments.addShort")}
            </button>
          )}
        </>
      )}
    </div>,
    document.body,
  );

  // The popover is a sibling of the toolbar, not a child: events inside it bubble through the
  // React tree, and the toolbar keeps the editor's selection by cancelling every mouse down.
  return (
    <>
      {toolbar}
      {palette !== null && (
        <TextColorPicker
          anchor={palette.anchor}
          anchorElement={palette.opener}
          textColor={state.textColor}
          highlight={state.highlight}
          onSelectTextColor={pickTextColor}
          onSelectHighlight={pickHighlight}
          onClose={() => {
            setPalette(null);
          }}
        />
      )}
    </>
  );
}

interface LinkFormProps {
  initial: string;
  canRemove: boolean;
  onApply: (href: string) => void;
  onRemove: () => void;
  onCancel: () => void;
}

/** The URL field of the link button: Enter applies, Escape cancels. */
function LinkForm({ initial, canRemove, onApply, onRemove, onCancel }: LinkFormProps) {
  const [value, setValue] = useState(initial);
  const [error, setError] = useState(false);
  const inputId = useId();

  function submit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    const href = normalizeHref(value);
    if (href === "") {
      if (canRemove) {
        onRemove();
      } else {
        onCancel();
      }
      return;
    }
    if (!isExternalHref(href)) {
      setError(true);
      return;
    }
    onApply(href);
  }

  return (
    <form className="link-form" noValidate onSubmit={submit}>
      <label className="visually-hidden" htmlFor={inputId}>
        {t("format.linkLabel")}
      </label>
      <input
        id={inputId}
        className="input link-input"
        type="url"
        value={value}
        placeholder={t("format.linkPlaceholder")}
        aria-invalid={error}
        autoFocus
        autoComplete="off"
        onChange={(event) => {
          setValue(event.target.value);
          setError(false);
        }}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            onCancel();
          }
        }}
      />
      <button type="submit" className="button">
        {t("format.linkApply")}
      </button>
      {canRemove && (
        <button type="button" className="button" onClick={onRemove}>
          {t("format.linkRemove")}
        </button>
      )}
      {error && (
        <p className="error-text link-error" role="alert">
          {t("format.linkInvalid")}
        </p>
      )}
    </form>
  );
}
