// The view of a template button (ADR-0056, `template-button.ts`): the button "+ label" that inserts a copy of the blocks it
// holds, a fold-out "Edit content" that shows those blocks in a frame (open while the button is new, that is while it holds
// one empty paragraph), and a settings popover for the label and where the copy goes. A read-only editor shows the button
// disabled and no fold-out: the content stays in the page, hidden.

import type { Node as PmNode } from "@tiptap/pm/model";
import { Selection } from "@tiptap/pm/state";
import { NodeViewContent, NodeViewWrapper, useEditorState } from "@tiptap/react";
import type { ReactNodeViewProps } from "@tiptap/react";
import { useState } from "react";
import type { KeyboardEvent } from "react";

import { usePeople } from "../comments-hooks";
import { anchorBelow, FloatingPanel } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { ChevronIcon, MoreIcon, PlusIcon } from "../components/icons";
import { t } from "../i18n";
import { clickTemplateButton, hasRelativePerson, meOf } from "./template-button-copy";
import type { TemplateMe } from "./template-button-copy";
import { labelOf, positionOf, TEMPLATE_BUTTON_LABEL_MAX_CHARS } from "./template-button";
import type { TemplateButtonOptions, TemplateButtonPosition } from "./template-button";

/** A button that was just made holds one empty paragraph: its content is open to be written. */
function isNew(button: PmNode): boolean {
  const first = button.firstChild;
  return button.childCount === 1 && first?.type.name === "paragraph" && first.content.size === 0;
}

export function TemplateButtonView({ node, editor, extension, getPos, updateAttributes }: ReactNodeViewProps) {
  const options = extension.options as TemplateButtonOptions;
  // The binding can turn the editor read-only after the view was drawn (content from a newer version).
  const editable = useEditorState({ editor, selector: ({ editor: current }) => current.isEditable });
  const label = labelOf(node.attrs.label);
  const shown = label === "" ? t("templateButton.defaultLabel") : label;
  const position = positionOf(node.attrs.position);
  // Who "me" is: the people of the space are read once, as the person mention's chip does.
  const people = usePeople(options.spaceId);
  const [open, setOpen] = useState(() => isNew(node));
  const [settings, setSettings] = useState<{ anchor: Anchor; opener: Element } | null>(null);
  const framed = editable && open;

  function run(me: TemplateMe | undefined): void {
    const pos = getPos();
    if (pos !== undefined && editor.isEditable) {
      clickTemplateButton(editor, pos, me);
    }
  }

  /** The click: the copy goes into the page; a mention of "me" waits for the people when they are not read yet. */
  function insert(): void {
    const pos = getPos();
    if (pos === undefined || !editor.isEditable) {
      return;
    }
    const me = meOf(people);
    const current = editor.state.doc.nodeAt(pos);
    if (me === undefined && current !== null && options.spaceId !== null && hasRelativePerson(current)) {
      options
        .findPeople()
        .then((list) => {
          run(meOf(list));
        })
        .catch(() => {
          run(undefined);
        });
      return;
    }
    run(me);
  }

  /**
   * Folds the content away. A caret inside it would be in hidden content, so it goes to the text after the button, or before it
   * when the button ends the page (and stays when there is no other text).
   */
  function toggle(): void {
    if (open) {
      const pos = getPos();
      if (pos !== undefined) {
        const end = pos + node.nodeSize;
        const { from, to } = editor.state.selection;
        if (from > pos && to < end) {
          const { doc } = editor.state;
          const elsewhere = Selection.findFrom(doc.resolve(end), 1, true) ?? Selection.findFrom(doc.resolve(pos), -1, true);
          if (elsewhere !== null) {
            editor.view.dispatch(editor.state.tr.setSelection(elsewhere));
          }
        }
      }
    }
    setOpen(!open);
  }

  return (
    <NodeViewWrapper className="template-button-body" data-position={position}>
      <div className="template-button-bar" contentEditable={false}>
        <button
          type="button"
          className="template-button-trigger"
          disabled={!editable}
          title={t(position === "above" ? "templateButton.insertsAbove" : "templateButton.insertsBelow")}
          onClick={insert}
        >
          <span className="template-button-plus" aria-hidden="true">
            <PlusIcon />
          </span>
          <span className="template-button-label">{shown}</span>
        </button>
        {editable && (
          <>
            <button type="button" className="template-button-fold" aria-expanded={open} onClick={toggle}>
              <span className="template-button-chevron" aria-hidden="true">
                <ChevronIcon />
              </span>
              {t("templateButton.content")}
            </button>
            <button
              type="button"
              className="template-button-settings"
              aria-label={t("templateButton.settings")}
              title={t("templateButton.settings")}
              aria-haspopup="dialog"
              aria-expanded={settings !== null}
              onClick={(event) => {
                const opener = event.currentTarget;
                setSettings((current) => (current === null ? { anchor: anchorBelow(opener), opener } : null));
              }}
            >
              <MoreIcon />
            </button>
          </>
        )}
      </div>
      <div className="template-button-frame" hidden={!framed} role="group" aria-label={t("templateButton.contentHint")}>
        <NodeViewContent className="template-button-content" />
      </div>
      {editable && settings !== null && (
        <TemplateButtonSettings
          anchor={settings.anchor}
          opener={settings.opener}
          label={label}
          position={position}
          onLabel={(next) => {
            updateAttributes({ label: next });
          }}
          onPosition={(next) => {
            updateAttributes({ position: next });
          }}
          onClose={() => {
            setSettings(null);
          }}
        />
      )}
    </NodeViewWrapper>
  );
}

interface SettingsProps {
  anchor: Anchor;
  opener: Element;
  label: string;
  position: TemplateButtonPosition;
  onLabel: (label: string) => void;
  onPosition: (position: TemplateButtonPosition) => void;
  onClose: () => void;
}

/** The label and the place of the copy. The label is kept when the field loses focus, on Enter, and when the popover closes. */
function TemplateButtonSettings({ anchor, opener, label, position, onLabel, onPosition, onClose }: SettingsProps) {
  const [draft, setDraft] = useState(label);

  function commit(): void {
    const next = labelOf(draft.trim());
    if (next !== label) {
      onLabel(next);
    }
  }

  function close(): void {
    commit();
    onClose();
  }

  return (
    <FloatingPanel label={t("templateButton.settings")} className="template-button-settings-panel" anchor={anchor} anchorElement={opener} onClose={close}>
      <label className="template-button-field">
        <span>{t("templateButton.label")}</span>
        <input
          type="text"
          value={draft}
          maxLength={TEMPLATE_BUTTON_LABEL_MAX_CHARS}
          placeholder={t("templateButton.defaultLabel")}
          onChange={(event) => {
            setDraft(event.target.value);
          }}
          onBlur={commit}
          onKeyDown={(event: KeyboardEvent<HTMLInputElement>) => {
            if (event.key === "Enter") {
              event.preventDefault();
              close();
            }
          }}
        />
      </label>
      <fieldset className="template-button-position">
        <legend>{t("templateButton.insert")}</legend>
        {(["below", "above"] as const).map((choice) => (
          <label key={choice} className="template-button-choice">
            <input
              type="radio"
              name="template-button-position"
              checked={position === choice}
              onChange={() => {
                onPosition(choice);
              }}
            />
            <span>{t(choice === "below" ? "templateButton.insertBelow" : "templateButton.insertAbove")}</span>
          </label>
        ))}
      </fieldset>
    </FloatingPanel>
  );
}
