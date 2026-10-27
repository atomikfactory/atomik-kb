import { TAGS_PROPERTY_ID, isComputedPropertyKind, propertyKindHasOptions, toCoreError } from "@atomik-kb/core-client";
import type {
  PageInfo,
  PropertyDef,
  PropertyId,
  PropertyOption,
  PropertyValue,
  TemplatePlaceholder,
} from "@atomik-kb/core-client";
import { useEffect, useLayoutEffect, useRef, useState } from "react";

import { t } from "../i18n";
import { KIND_LABELS } from "../property-format";
import { useAppStore } from "../store-context";
import { templateRootOf } from "../template-tree";
import { ConfirmDialog } from "./ConfirmDialog";
import { FloatingPanel, anchorBelow } from "./FloatingPanel";
import type { Anchor } from "./FloatingPanel";
import { FormulaEditorHost } from "./FormulaEditor";
import { InlineEdit } from "./InlineEdit";
import { Menu } from "./Menu";
import type { MenuItem } from "./Menu";
import { NewPropertyDialog } from "./NewPropertyForm";
import { PropertiesDialog } from "./PropertiesDialog";
import { DeleteOptionConfirm, DeletePropertyConfirm } from "./PropertyConfirms";
import { PropertyEditor } from "./PropertyEditors";
import { KindGlyph, TagGlyph } from "./property-icons";
import { PlaceholderMenuButton, PlaceholderValue, placeholdersFor } from "./TemplatePlaceholders";

interface Popup {
  anchor: Anchor;
  trigger: Element;
}

/** What the panel is asking the person to confirm. */
type Question =
  | { kind: "hide"; def: PropertyDef; returnTo: Element }
  | { kind: "delete"; def: PropertyDef; returnTo: Element }
  | { kind: "deleteOption"; def: PropertyDef; option: PropertyOption };

export interface PropertiesPanelProps {
  page: PageInfo;
  /** Show the values and change nothing; by default, a page in the trash. */
  readOnly?: boolean;
}

interface RowProps {
  def: PropertyDef;
  page: PageInfo;
  value: PropertyValue | undefined;
  /** The placeholder of the property on this page (ADR-0056); only on a page in a template. */
  placeholder: TemplatePlaceholder | undefined;
  /** The page is a template or inside one: its date and person properties offer placeholders. */
  inTemplate: boolean;
  readOnly: boolean;
  autoFocus: boolean;
  canMoveUp: boolean;
  canMoveDown: boolean;
  onCommand: (def: PropertyDef, command: string, trigger: Element) => void;
  onRename: (def: PropertyDef, name: string) => Promise<void>;
  onDeleteOption: (def: PropertyDef, option: PropertyOption) => void;
  onPlaceholder: (def: PropertyDef, placeholder: TemplatePlaceholder | null) => void;
}

/** One property of the page: its name (a menu of what can be done to the property) and its value editor. */
function PropertyRow({
  def,
  page,
  value,
  placeholder,
  inTemplate,
  readOnly,
  autoFocus,
  canMoveUp,
  canMoveDown,
  onCommand,
  onRename,
  onDeleteOption,
  onPlaceholder,
}: RowProps) {
  const setPageProperty = useAppStore((state) => state.setPageProperty);
  const [menu, setMenu] = useState<Popup | null>(null);
  const [renaming, setRenaming] = useState(false);
  const nameButton = useRef<HTMLButtonElement>(null);
  const refocusName = useRef(false);
  const isTags = def.id === TAGS_PROPERTY_ID;
  const computed = isComputedPropertyKind(def.kind);
  const hasValue = value !== undefined || placeholder !== undefined;

  // The rename field is gone: the name it replaced takes focus back.
  useLayoutEffect(() => {
    if (refocusName.current && nameButton.current !== null) {
      refocusName.current = false;
      if (document.activeElement === null || document.activeElement === document.body) {
        nameButton.current.focus();
      }
    }
  });

  const items: MenuItem[] = [{ id: "rename", label: t("properties.menu.rename") }];
  if (propertyKindHasOptions(def.kind)) {
    items.push({ id: "options", label: t("properties.menu.options") });
  } else if (def.kind === "rollup") {
    items.push({ id: "options", label: t("properties.menu.settings") });
  } else if (def.kind === "formula") {
    items.push({ id: "editFormula", label: t("formula.menu.edit") });
  }
  items.push(
    {
      id: "hide",
      label: t("properties.menu.hide"),
      disabled: isTags || computed,
      title: isTags ? t("properties.menu.hideTags") : computed ? t("properties.menu.hideComputed") : undefined,
    },
    { id: "up", label: t("properties.menu.up"), disabled: !canMoveUp },
    { id: "down", label: t("properties.menu.down"), disabled: !canMoveDown },
    {
      id: "delete",
      label: t("properties.menu.delete"),
      danger: true,
      disabled: isTags,
      title: isTags ? t("properties.menu.deleteTags") : undefined,
    },
  );

  return (
    <li className="property-row" aria-label={def.name} data-kind={def.kind} data-empty={hasValue ? undefined : ""}>
      <div className="property-name">
        {renaming ? (
          <InlineEdit
            className="property-rename"
            value={def.name}
            label={t("properties.rename.label")}
            onCommit={(name) => onRename(def, name)}
            onDone={() => {
              refocusName.current = true;
              setRenaming(false);
            }}
          />
        ) : readOnly ? (
          <span className="property-name-text">
            <span className="property-glyph">{isTags ? <TagGlyph /> : <KindGlyph kind={def.kind} />}</span>
            <span>{def.name}</span>
          </span>
        ) : (
          <button
            ref={nameButton}
            type="button"
            className="property-name-button"
            aria-haspopup="menu"
            aria-expanded={menu !== null}
            title={t("properties.menu.title", { kind: t(KIND_LABELS[def.kind]) })}
            onClick={(event) => {
              const trigger = event.currentTarget;
              setMenu((current) => (current === null ? { anchor: anchorBelow(trigger), trigger } : null));
            }}
          >
            <span className="property-glyph">{isTags ? <TagGlyph /> : <KindGlyph kind={def.kind} />}</span>
            <span>{def.name}</span>
          </button>
        )}
      </div>
      <div className={inTemplate ? "property-value has-placeholders" : "property-value"}>
        {placeholder !== undefined ? (
          <PlaceholderValue
            def={def}
            placeholder={placeholder}
            readOnly={readOnly}
            onClear={() => {
              onPlaceholder(def, null);
            }}
          />
        ) : (
          <>
            <PropertyEditor
              def={def}
              spaceId={page.spaceId}
              value={value}
              readOnly={readOnly}
              autoFocus={autoFocus}
              commit={(next) => setPageProperty(page.id, def.id, next)}
              onDeleteOption={(option) => {
                onDeleteOption(def, option);
              }}
            />
            {inTemplate && !readOnly && placeholdersFor(def.kind).length > 0 && (
              <PlaceholderMenuButton
                def={def}
                onChoose={(choice) => {
                  onPlaceholder(def, choice);
                }}
              />
            )}
          </>
        )}
      </div>
      {menu !== null && (
        <Menu
          label={t("properties.menu.label", { name: def.name })}
          items={items}
          anchor={menu.anchor}
          anchorElement={menu.trigger}
          onSelect={(command) => {
            if (command === "rename") {
              setRenaming(true);
            } else {
              onCommand(def, command, menu.trigger);
            }
          }}
          onClose={() => {
            setMenu(null);
          }}
        />
      )}
    </li>
  );
}

/**
 * The properties of the open page, between its title and its text: one row per property that
 * has a value on the page, in the order of the space's schema (Tags first, and always there, with
 * "Add tags" while it is empty), each a name that opens a menu (rename, options, hide, delete, move)
 * and the editor of its kind. "Add a property" lists the schema's other properties and "New
 * property...". Nothing is changed on a page in the trash (`readOnly`). On a page in a template (ADR-0056) a date or person
 * property can instead be filled in when a page is made from the template ("Today (when created)", "Now (when created)", "Me
 * (whoever creates)"): its row shows the placeholder as a chip with a "filled in" mark and a Clear button.
 */
export function PropertiesPanel({ page, readOnly }: PropertiesPanelProps) {
  const locked = readOnly ?? page.trashedAt !== undefined;
  const spaceId = page.spaceId;
  const schema = useAppStore((state) => state.propertySchemas[spaceId]);
  const loadPropertySchema = useAppStore((state) => state.loadPropertySchema);
  const setPageProperty = useAppStore((state) => state.setPageProperty);
  const moveProperty = useAppStore((state) => state.moveProperty);
  const renameProperty = useAppStore((state) => state.renameProperty);
  const setTemplatePlaceholder = useAppStore((state) => state.setTemplatePlaceholder);
  // In a template (the page, or one of its ancestors, has the flag; ADR-0056) a property can be filled in when a page is made from it.
  const inTemplate = useAppStore((state) => templateRootOf(state.tree, page.id) !== null);
  // Properties added to the page that have no value yet: their rows show so that one can be typed.
  const [pending, setPending] = useState<readonly PropertyId[]>([]);
  const [focusId, setFocusId] = useState<PropertyId | null>(null);
  const [addMenu, setAddMenu] = useState<Popup | null>(null);
  const [creating, setCreating] = useState<{ returnTo: Element | null } | null>(null);
  const [question, setQuestion] = useState<Question | null>(null);
  const [managing, setManaging] = useState<{ propertyId: PropertyId; returnTo: Element } | null>(null);
  const [editingFormula, setEditingFormula] = useState<{ def: PropertyDef; returnTo: Element; pages: readonly PageInfo[] } | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    loadPropertySchema(spaceId);
  }, [loadPropertySchema, spaceId]);

  if (schema === undefined) {
    return null;
  }
  const order: readonly PropertyDef[] = schema;
  const values = new Map(page.properties ?? []);
  // A page that left its template keeps its map but ignores it until it is in a template again.
  const placeholders = new Map(inTemplate ? (page.templatePlaceholders ?? []) : []);
  // A pending property that got its value is a row like any other from then on.
  if (pending.some((id) => values.has(id))) {
    setPending(pending.filter((id) => !values.has(id)));
  }
  // Computed properties (created and last edited time and device) show on every page.
  const rows = order.filter(
    (def) =>
      values.has(def.id) ||
      placeholders.has(def.id) ||
      pending.includes(def.id) ||
      isComputedPropertyKind(def.kind) ||
      (def.id === TAGS_PROPERTY_ID && !locked),
  );
  const addable = order.filter(
    (def) =>
      def.id !== TAGS_PROPERTY_ID &&
      !isComputedPropertyKind(def.kind) &&
      !values.has(def.id) &&
      !placeholders.has(def.id) &&
      !pending.includes(def.id),
  );
  if (locked && rows.length === 0) {
    return null;
  }

  function fail(message: string): void {
    setNotice(message);
  }

  /** Shows a property on the page so that a value can be given: a checkbox starts unchecked, the others focused. */
  function offerValue(def: PropertyDef): void {
    if (def.kind === "checkbox") {
      setPageProperty(page.id, def.id, { kind: "checkbox", value: false }).catch((failure: unknown) => {
        fail(t("properties.error.save", { message: toCoreError(failure).message }));
      });
      return;
    }
    setPending((current) => (current.includes(def.id) ? current : [...current, def.id]));
    setFocusId(def.id);
  }

  async function renameRow(def: PropertyDef, name: string): Promise<void> {
    setNotice(null);
    try {
      await renameProperty(spaceId, def.id, name);
    } catch (failure) {
      fail(t("properties.error.rename", { message: toCoreError(failure).message }));
    }
  }

  /** Sets or clears the placeholder of a property of this template page; a refusal shows under the properties. */
  function changePlaceholder(def: PropertyDef, placeholder: TemplatePlaceholder | null): void {
    setNotice(null);
    setTemplatePlaceholder(page.id, def.id, placeholder).catch((failure: unknown) => {
      fail(t("template.error.placeholder", { message: toCoreError(failure).message }));
    });
  }

  function move(def: PropertyDef, before: PropertyId | undefined): void {
    setNotice(null);
    moveProperty(spaceId, def.id, before).catch((failure: unknown) => {
      fail(t("properties.error.move", { message: toCoreError(failure).message }));
    });
  }

  /** Where a property can go with the rows that show: in front of the row above, or after the row below. */
  function neighbours(def: PropertyDef): { up: PropertyId | null; down: { before: PropertyId | undefined } | null } {
    const at = rows.findIndex((row) => row.id === def.id);
    const above = rows[at - 1];
    const below = rows[at + 1];
    const inSchema = order.findIndex((entry) => entry.id === def.id);
    // Tags is always first, and a property already next to it cannot go in front of it.
    const up = def.id === TAGS_PROPERTY_ID || above === undefined || inSchema <= 1 ? null : above.id;
    let down: { before: PropertyId | undefined } | null = null;
    if (def.id !== TAGS_PROPERTY_ID && below !== undefined) {
      down = { before: order[order.findIndex((entry) => entry.id === below.id) + 1]?.id };
    }
    return { up, down };
  }

  function command(def: PropertyDef, id: string, trigger: Element): void {
    setNotice(null);
    if (id === "options") {
      setManaging({ propertyId: def.id, returnTo: trigger });
    } else if (id === "editFormula") {
      // The preview runs on this page (the panel may be a peek's, not the page in front).
      setEditingFormula({ def, returnTo: trigger, pages: [page] });
    } else if (id === "hide") {
      if (values.has(def.id)) {
        setQuestion({ kind: "hide", def, returnTo: trigger });
      } else if (placeholders.has(def.id)) {
        changePlaceholder(def, null); // a placeholder is the page's value of the property: hiding it clears it
      } else {
        setPending((current) => current.filter((entry) => entry !== def.id));
      }
    } else if (id === "delete") {
      setQuestion({ kind: "delete", def, returnTo: trigger });
    } else if (id === "up") {
      const { up } = neighbours(def);
      if (up !== null) {
        move(def, up);
      }
    } else if (id === "down") {
      const { down } = neighbours(def);
      if (down !== null) {
        move(def, down.before);
      }
    }
  }

  return (
    <section className="properties" aria-label={t("properties.title")}>
      <ul className="properties-list">
        {rows.map((def) => {
          const moves = neighbours(def);
          return (
            <PropertyRow
              key={`${page.id}:${def.id}`}
              def={def}
              page={page}
              value={values.get(def.id)}
              placeholder={placeholders.get(def.id)}
              inTemplate={inTemplate}
              readOnly={locked}
              autoFocus={focusId === def.id}
              canMoveUp={moves.up !== null}
              canMoveDown={moves.down !== null}
              onCommand={command}
              onRename={renameRow}
              onDeleteOption={(owner, option) => {
                setQuestion({ kind: "deleteOption", def: owner, option });
              }}
              onPlaceholder={changePlaceholder}
            />
          );
        })}
      </ul>
      {!locked && (
        <button
          type="button"
          className="properties-add"
          aria-haspopup="menu"
          aria-expanded={addMenu !== null}
          onClick={(event) => {
            const trigger = event.currentTarget;
            setAddMenu((current) => (current === null ? { anchor: anchorBelow(trigger), trigger } : null));
          }}
        >
          {t("properties.add")}
        </button>
      )}
      {notice !== null && (
        <p className="error-text properties-notice" role="alert">
          {notice}
        </p>
      )}

      {addMenu !== null && (
        <FloatingPanel
          role="menu"
          label={t("properties.add")}
          className="properties-add-menu"
          anchor={addMenu.anchor}
          anchorElement={addMenu.trigger}
          onClose={() => {
            setAddMenu(null);
          }}
        >
          {addable.map((def) => (
            <button
              key={def.id}
              type="button"
              role="menuitem"
              className="menu-item properties-add-item"
              onClick={() => {
                setAddMenu(null);
                offerValue(def);
              }}
            >
              <span className="property-glyph">
                <KindGlyph kind={def.kind} />
              </span>
              <span className="properties-add-name">{def.name}</span>
              <span className="muted">{t(KIND_LABELS[def.kind])}</span>
            </button>
          ))}
          <button
            type="button"
            role="menuitem"
            className="menu-item properties-add-item"
            onClick={() => {
              setCreating({ returnTo: addMenu.trigger });
              setAddMenu(null);
            }}
          >
            {t("properties.new.menu")}
          </button>
        </FloatingPanel>
      )}
      {creating !== null && (
        <NewPropertyDialog
          spaceId={spaceId}
          onCreated={offerValue}
          onClose={() => {
            setCreating(null);
          }}
        />
      )}
      {question?.kind === "hide" && (
        <ConfirmDialog
          returnFocus={question.returnTo}
          title={t("properties.hide.title", { name: question.def.name })}
          body={t("properties.hide.body", { name: question.def.name })}
          confirmLabel={t("properties.hide.confirm")}
          onConfirm={async () => {
            try {
              await setPageProperty(page.id, question.def.id, null);
            } catch (failure) {
              fail(t("properties.error.save", { message: toCoreError(failure).message }));
            }
          }}
          onClose={() => {
            setQuestion(null);
          }}
        />
      )}
      {question?.kind === "delete" && (
        <DeletePropertyConfirm
          spaceId={spaceId}
          def={question.def}
          returnFocus={question.returnTo}
          onError={fail}
          onClose={() => {
            setQuestion(null);
          }}
        />
      )}
      {question?.kind === "deleteOption" && (
        <DeleteOptionConfirm
          spaceId={spaceId}
          def={question.def}
          option={question.option}
          onError={fail}
          onClose={() => {
            setQuestion(null);
          }}
        />
      )}
      {editingFormula !== null && (
        <FormulaEditorHost
          spaceId={spaceId}
          def={editingFormula.def}
          previewPages={editingFormula.pages}
          returnFocus={editingFormula.returnTo}
          onSaved={() => {
            setEditingFormula(null);
          }}
          onClose={() => {
            setEditingFormula(null);
          }}
        />
      )}
      {managing !== null && (
        <PropertiesDialog
          spaceId={spaceId}
          initialPropertyId={managing.propertyId}
          returnFocus={managing.returnTo}
          onClose={() => {
            setManaging(null);
          }}
        />
      )}
    </section>
  );
}
