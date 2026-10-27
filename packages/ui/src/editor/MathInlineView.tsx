// The chip of an inline equation: the formula typeset by KaTeX in the line of text (the source in
// place of it while KaTeX loads or when it does not parse, in the danger colour with KaTeX's message as
// the tooltip), a sigma while the source is empty. A click, or Enter on the chip selected with the arrow
// keys, opens `MathInlineEditor` to change it; a read-only editor shows the chip and nothing more.

import { NodeViewWrapper, useEditorState } from "@tiptap/react";
import type { ReactNodeViewProps } from "@tiptap/react";
import { useState, useSyncExternalStore } from "react";

import { anchorBelow } from "../components/FloatingPanel";
import { EquationIcon } from "../components/icons";
import { t } from "../i18n";
import type { MathInlineStorage } from "./math-inline";
import { useTypeset } from "./math-typeset";
import { MathInlineEditor } from "./MathInlineEditor";

export function MathInlineView({ node, editor, extension, getPos, updateAttributes }: ReactNodeViewProps) {
  const id = typeof node.attrs.id === "string" ? node.attrs.id : null;
  const latex = typeof node.attrs.latex === "string" ? node.attrs.latex : "";
  // The binding can turn the editor read-only after the view was drawn (content from a newer version).
  const editable = useEditorState({ editor, selector: ({ editor: current }) => current.isEditable });
  const requests = (extension.storage as MathInlineStorage).requests;
  const openFor = useSyncExternalStore(requests.subscribe, requests.getSnapshot);
  const [element, setElement] = useState<HTMLElement | null>(null);
  const open = editable && id !== null && openFor === id && element !== null;
  const typeset = useTypeset(latex);

  /** Puts the caret right after the equation and gives the page the focus back. */
  function caretAfter(): void {
    const pos = getPos();
    if (pos !== undefined) {
      editor.commands.setTextSelection(pos + node.nodeSize);
    }
    editor.commands.focus();
  }

  let body;
  switch (typeset.state) {
    case "ready":
      // KaTeX's own output (trust: false), never the document's text.
      body = <span className="math-inline-render" dangerouslySetInnerHTML={{ __html: typeset.markup }} />;
      break;
    case "empty":
      body = (
        <span className="math-inline-placeholder" aria-hidden="true">
          <EquationIcon />
        </span>
      );
      break;
    default:
      body = <span className="math-inline-source">{latex}</span>;
  }

  return (
    <NodeViewWrapper as="span" ref={setElement} className="math-inline-wrapper">
      <span
        className={`math-inline is-${typeset.state}`}
        role={editable ? "button" : undefined}
        aria-haspopup={editable ? "dialog" : undefined}
        aria-expanded={editable ? open : undefined}
        aria-label={editable ? (latex.trim() === "" ? t("mathInline.editEmpty") : t("mathInline.edit", { latex })) : undefined}
        title={typeset.state === "error" ? typeset.message : undefined}
        onClick={() => {
          if (editable && id !== null) {
            if (openFor === id) {
              requests.close(id);
            } else {
              requests.open(id);
            }
          }
        }}
      >
        {body}
      </span>
      {open && (
        <MathInlineEditor
          anchor={anchorBelow(element)}
          anchorElement={element}
          returnFocus={editor.view.dom}
          latex={latex}
          onApply={(next) => {
            updateAttributes({ latex: next });
            caretAfter();
          }}
          onRemove={() => {
            const pos = getPos();
            requests.close();
            if (pos !== undefined) {
              editor.view.dispatch(editor.state.tr.delete(pos, pos + node.nodeSize));
            }
            editor.commands.focus();
          }}
          onClose={() => {
            requests.close();
          }}
        />
      )}
    </NodeViewWrapper>
  );
}
