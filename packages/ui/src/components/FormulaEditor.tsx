// The formula editor (ADR-0055, `docs/architecture/formulas.md` section 9): a dialog to write the expression of a formula
// property. The code field is a textarea over a mirror that draws the same text highlighted (the tokens of the engine's
// lexer) with the first error underlined; completion proposes the properties (as `prop("Name")`), the functions and
// `current` and `index` (Up and Down choose, Tab or Enter accepts, Esc closes the list and only then the dialog); a
// reference lists the functions by category with their signature, a short description and an example; the result type
// and a live preview on a page (the engine evaluating over the pages the UI holds) say what the formula does. The text
// is written with property names and saved with their ids (`formulaToStored`); Save is refused while there is an error.
// `FormulaEditorHost` is the one the openers use: it reads the schema, the page tree and the people from the store and saves
// through `setPropertyConfig` (an existing property) or `createProperty` (a new one: it exists only once saved).

import { evaluateFormula, FORMULA_FUNCTIONS, formulaToDisplay, toCoreError, tokenizeFormula } from "@atomik-kb/core-client";
import type { FormulaHost, FormulaResult, FormulaSchema, PageId, PageInfo, PropertyDef, SpaceId } from "@atomik-kb/core-client";
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { ChangeEvent, KeyboardEvent, RefObject } from "react";

import { FormulaValue } from "../editor/PageListValue";
import { useToday } from "../editor/today";
import { usePeople } from "../comments-hooks";
import { formulaExpressionOf } from "../formula-format";
import { t } from "../i18n";
import { useAppStore } from "../store-context";
import { Modal } from "./Modal";
import {
  analyseFormula,
  applyCompletion,
  ambiguousNames,
  buildFormulaHost,
  choosePreviewPage,
  completionAt,
  formulaErrorMessage,
  formulaSchemaOf,
  formulaTypeLabel,
  functionGroups,
  FUNCTION_DESCRIPTIONS,
  highlightSegments,
  pagesOfTree,
  previewEnv,
} from "./formula-editor-model";
import type { CompletionItem } from "./formula-editor-model";

/** The page the preview runs on, with what it reads. */
export interface FormulaPreview {
  readonly host: FormulaHost;
  readonly page: PageId;
  readonly title: string;
}

export interface FormulaEditorProps {
  /** The property's name, for the title of the dialog. */
  name: string;
  /** The properties the formula is checked against. */
  schema: FormulaSchema;
  /** The property's own id (none for a property that does not exist yet): a formula that reads itself is a cycle. */
  selfId?: string;
  /** The stored expression the editor starts from (`""` for a new formula). */
  initial: string;
  /** Where the preview runs; `null` when the space has no page. */
  preview: FormulaPreview | null;
  /** The name of the title column, which `prop("…")` may name. */
  titleLabel: string;
  /** Saves the stored expression; a rejection is shown in the dialog and keeps it open. The owner closes the dialog on success. */
  onSave: (stored: string) => Promise<void>;
  onCancel: () => void;
  /** Gets focus back when the dialog closes. */
  returnFocus?: RefObject<HTMLElement | null>;
}

/** The preview's result on its page: `undefined` while the formula has an error, `null` for no value. */
function usePreviewResult(stored: string | undefined, preview: FormulaPreview | null): FormulaResult | null | undefined {
  return useMemo(() => {
    if (stored === undefined || preview === null) {
      return undefined;
    }
    try {
      return evaluateFormula(stored, preview.page, preview.host, previewEnv());
    } catch {
      return undefined;
    }
  }, [stored, preview]);
}

/** The reference of the functions: a filter, then the categories with each function's signatures, description and example. */
function FunctionReference({ onInsert }: { onInsert: (name: string, hasArguments: boolean) => void }) {
  const [query, setQuery] = useState("");
  const filterId = useId();
  const groups = useMemo(
    () => functionGroups(FORMULA_FUNCTIONS, query, (name) => (FUNCTION_DESCRIPTIONS[name] === undefined ? "" : t(FUNCTION_DESCRIPTIONS[name]))),
    [query],
  );
  return (
    <section className="formula-reference" aria-label={t("formula.reference.title")}>
      <h3 className="manage-options-title">{t("formula.reference.title")}</h3>
      <label className="visually-hidden" htmlFor={filterId}>
        {t("formula.reference.filter")}
      </label>
      <input
        id={filterId}
        type="search"
        className="input formula-reference-filter"
        placeholder={t("formula.reference.filter")}
        autoComplete="off"
        value={query}
        onChange={(event) => {
          setQuery(event.target.value);
        }}
      />
      <div className="formula-reference-list">
        {groups.length === 0 && <p className="muted">{t("formula.reference.none")}</p>}
        {groups.map((group) => (
          <div key={group.id} role="group" aria-label={t(group.label)}>
            <h4 className="formula-reference-category" aria-hidden="true">
              {t(group.label)}
            </h4>
            <ul className="formula-reference-items">
              {group.items.map((fn) => {
                const description = FUNCTION_DESCRIPTIONS[fn.name];
                return (
                  <li key={fn.name}>
                    <button
                      type="button"
                      className="formula-reference-item"
                      title={t("formula.reference.insert", { name: fn.name })}
                      onClick={() => {
                        onInsert(fn.name, !(fn.signatures[0] ?? "").startsWith(`${fn.name}()`));
                      }}
                    >
                      <code className="formula-reference-name">{fn.name}</code>
                      {fn.signatures.map((signature) => (
                        <code key={signature} className="formula-reference-signature">
                          {signature}
                        </code>
                      ))}
                      {description !== undefined && <span className="formula-reference-description">{t(description)}</span>}
                      <code className="formula-reference-example">{fn.example}</code>
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>
        ))}
      </div>
    </section>
  );
}

/** The dialog that edits one formula. */
export function FormulaEditor({ name, schema, selfId, initial, preview, titleLabel, onSave, onCancel, returnFocus }: FormulaEditorProps) {
  const titleId = useId();
  const introId = useId();
  const codeId = useId();
  const listId = useId();
  const statusId = useId();
  const textarea = useRef<HTMLTextAreaElement>(null);
  const pendingCaret = useRef<number | null>(null);
  const today = useToday();
  const [text, setText] = useState(() => (initial === "" ? "" : formulaToDisplay(initial, schema, titleLabel)));
  const [caret, setCaret] = useState(text.length);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [chosen, setChosen] = useState<{ key: string; index: number }>({ key: "", index: 0 });
  const [busy, setBusy] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const analysis = useMemo(() => analyseFormula(text, schema, selfId, titleLabel), [text, schema, selfId, titleLabel]);
  const errorRange = analysis.kind === "error" ? analysis.range : null;
  const segments = useMemo(() => highlightSegments(text, tokenizeFormula(text), errorRange), [text, errorRange]);
  const stored = analysis.kind === "ok" ? analysis.stored : undefined;
  const result = usePreviewResult(stored, preview);
  const sources = useMemo(
    () => ({ properties: schema.properties(), titleLabel, functions: FORMULA_FUNCTIONS }),
    [schema, titleLabel],
  );
  const found = useMemo(() => completionAt(text, Math.min(caret, text.length), sources), [text, caret, sources]);
  const completion = found !== null && found.key !== dismissed ? found : null;
  const active = completion === null ? 0 : chosen.key === completion.key ? Math.min(chosen.index, completion.items.length - 1) : 0;
  const ambiguous = useMemo(
    () =>
      ambiguousNames(
        text,
        schema.properties().map((property) => property.name),
        titleLabel,
      ),
    [text, schema, titleLabel],
  );

  // A caret set by a completion or an insertion waits for the textarea to hold the new text.
  useLayoutEffect(() => {
    const at = pendingCaret.current;
    if (at !== null && textarea.current !== null) {
      pendingCaret.current = null;
      textarea.current.setSelectionRange(at, at);
    }
  });

  function edit(next: string, at: number): void {
    pendingCaret.current = at;
    setText(next);
    setCaret(at);
    setSaveError(null);
  }

  function accept(item: CompletionItem): void {
    if (completion === null) {
      return;
    }
    const next = applyCompletion(text, completion, item);
    edit(next.text, next.caret);
    setDismissed(null);
    textarea.current?.focus();
  }

  /** A function of the reference written at the caret (over the selection), the caret inside its brackets when it takes arguments. */
  function insertFunction(fnName: string, hasArguments: boolean): void {
    const field = textarea.current;
    const from = field?.selectionStart ?? text.length;
    const to = field?.selectionEnd ?? from;
    const insert = `${fnName}()`;
    edit(`${text.slice(0, from)}${insert}${text.slice(to)}`, from + insert.length - (hasArguments ? 1 : 0));
    field?.focus();
  }

  async function save(): Promise<void> {
    if (analysis.kind !== "ok" || busy) {
      return;
    }
    setBusy(true);
    setSaveError(null);
    try {
      await onSave(analysis.stored);
    } catch (failure) {
      setSaveError(t("formula.error.save", { message: toCoreError(failure).message }));
      setBusy(false);
    }
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): void {
    if (event.nativeEvent.isComposing) {
      return;
    }
    if (completion !== null) {
      const count = completion.items.length;
      const item = completion.items[active];
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        setChosen({ key: completion.key, index: (active + (event.key === "ArrowDown" ? 1 : count - 1)) % count });
        return;
      }
      if ((event.key === "Enter" || (event.key === "Tab" && !event.shiftKey)) && item !== undefined) {
        event.preventDefault();
        event.stopPropagation();
        accept(item);
        return;
      }
      if (event.key === "Escape") {
        // The list closes first; the dialog only on the next Escape.
        event.preventDefault();
        event.stopPropagation();
        setDismissed(completion.key);
        return;
      }
    }
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      void save();
    }
  }

  function onChange(event: ChangeEvent<HTMLTextAreaElement>): void {
    setText(event.target.value);
    setCaret(event.target.selectionStart);
    setSaveError(null);
  }

  const optionId = (index: number): string => `${listId}-${String(index)}`;
  const message = analysis.kind === "error" ? formulaErrorMessage(analysis.error) : null;
  const statusText = analysis.kind === "ok" ? t("formula.result", { type: formulaTypeLabel(analysis.type) }) : null;

  return (
    <Modal labelledBy={titleId} describedBy={introId} onDismiss={onCancel} className="formula-dialog" initialFocus={textarea} returnFocus={returnFocus}>
      <h2 className="subtitle" id={titleId}>
        {t("formula.editor.title", { name })}
      </h2>
      <p className="muted" id={introId}>
        {t("formula.editor.intro")}
      </p>
      <div className="formula-editor">
        <div className="formula-main">
          <label className="visually-hidden" htmlFor={codeId}>
            {t("formula.editor.code")}
          </label>
          <div className="formula-code" data-invalid={analysis.kind === "error" ? "" : undefined}>
            <pre className="formula-mirror" aria-hidden="true">
              <code>
                {segments.map((segment, at) => (
                  <span
                    key={`${String(at)}:${segment.text}`}
                    className={[segment.kind === null ? "" : `formula-token formula-token-${segment.kind}`, segment.error ? "formula-token-error-range" : ""]
                      .filter((name) => name !== "")
                      .join(" ")}
                  >
                    {segment.text}
                  </span>
                ))}
                {"​"}
              </code>
            </pre>
            <textarea
              id={codeId}
              ref={textarea}
              className="formula-input"
              value={text}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              autoComplete="off"
              rows={4}
              role="combobox"
              aria-autocomplete="list"
              aria-expanded={completion !== null}
              aria-controls={completion === null ? undefined : listId}
              aria-activedescendant={completion === null ? undefined : optionId(active)}
              aria-invalid={analysis.kind === "error"}
              aria-describedby={statusId}
              onChange={onChange}
              onKeyDown={onKeyDown}
              onSelect={(event) => {
                setCaret(event.currentTarget.selectionStart);
              }}
              onClick={(event) => {
                setCaret(event.currentTarget.selectionStart);
              }}
              onKeyUp={(event) => {
                setCaret(event.currentTarget.selectionStart);
              }}
            />
          </div>
          {completion !== null && (
            <ul id={listId} role="listbox" className="formula-completions" aria-label={t("formula.completion.label")}>
              {completion.items.map((item, index) => (
                <li
                  key={item.id}
                  id={optionId(index)}
                  role="option"
                  aria-selected={index === active}
                  className="formula-completion"
                  data-kind={item.kind}
                  onMouseDown={(event) => {
                    // The textarea keeps focus.
                    event.preventDefault();
                  }}
                  onClick={() => {
                    accept(item);
                  }}
                >
                  <span className="formula-completion-label">{item.label}</span>
                  <span className="formula-completion-detail muted">{item.detail}</span>
                </li>
              ))}
            </ul>
          )}
          <div className="formula-status" id={statusId} role="status">
            {message !== null && <p className="error-text formula-error">{message}</p>}
            {statusText !== null && <p className="muted formula-type">{statusText}</p>}
            {analysis.kind === "empty" && <p className="muted formula-type">{t("formula.editor.empty")}</p>}
          </div>
          {ambiguous.map((ambiguousName) => (
            <p key={ambiguousName} className="formula-warning" role="note">
              {t("formula.warning.ambiguous", { name: ambiguousName })}
            </p>
          ))}
          <section className="formula-preview" aria-label={t("formula.preview.label")}>
            <h3 className="manage-options-title">{t("formula.preview.label")}</h3>
            {preview === null ? (
              <p className="muted">{t("formula.preview.none")}</p>
            ) : (
              <>
                <p className="muted formula-preview-page">{t("formula.preview.on", { title: preview.title === "" ? t("page.untitled") : preview.title })}</p>
                <div className="formula-preview-value">
                  {result === undefined ? (
                    // Nothing to show for an empty text; an error says why there is no result.
                    analysis.kind === "error" && <span className="muted">{t("formula.preview.waiting")}</span>
                  ) : result === null ? (
                    <span className="muted">{t("properties.empty")}</span>
                  ) : (
                    <FormulaValue result={result} today={today} variant="panel" />
                  )}
                </div>
              </>
            )}
          </section>
          {saveError !== null && (
            <p className="error-text" role="alert">
              {saveError}
            </p>
          )}
        </div>
        <FunctionReference onInsert={insertFunction} />
      </div>
      <div className="dialog-actions">
        <button type="button" className="button" onClick={onCancel}>
          {t("dialog.cancel")}
        </button>
        <button
          type="button"
          className="button button-primary"
          disabled={busy || analysis.kind !== "ok"}
          onClick={() => {
            void save();
          }}
        >
          {busy ? t("formula.editor.saving") : t("formula.editor.save")}
        </button>
      </div>
    </Modal>
  );
}

export interface FormulaEditorHostProps {
  spaceId: SpaceId;
  /** The formula property to edit; leave out to write the formula of a new property (`name`), created when saved. */
  def?: PropertyDef;
  /** The name of the new property. */
  name?: string;
  /** Pages the preview may run on first (a table offers its rows); by default the page in front, else the first page. */
  previewPages?: readonly PageInfo[];
  /** The control that opened the editor, which gets focus back. */
  returnFocus?: Element | null;
  /** The property was saved (changed or created) and the store knows it; the owner closes the editor. */
  onSaved: (property: PropertyDef) => void;
  /** The editor was cancelled. */
  onClose: () => void;
}

const NO_PAGES: readonly PageInfo[] = [];

/** The formula editor over the app: the schema, the pages and the people come from the store, and saving writes the property. */
export function FormulaEditorHost({ spaceId, def, name, previewPages = NO_PAGES, returnFocus, onSaved, onClose }: FormulaEditorHostProps) {
  const defs = useAppStore((state) => state.propertySchemas[spaceId]);
  const loadPropertySchema = useAppStore((state) => state.loadPropertySchema);
  const setPropertyConfig = useAppStore((state) => state.setPropertyConfig);
  const createProperty = useAppStore((state) => state.createProperty);
  const tree = useAppStore((state) => state.tree);
  const selectedPageId = useAppStore((state) => state.selectedPageId);
  const people = usePeople(spaceId);
  const returnRef = useRef<HTMLElement | null>(returnFocus instanceof HTMLElement ? returnFocus : null);

  useEffect(() => {
    loadPropertySchema(spaceId);
  }, [loadPropertySchema, spaceId]);

  const schema = useMemo(() => (defs === undefined ? null : formulaSchemaOf(defs)), [defs]);
  const pages = useMemo(() => (tree?.spaceId === spaceId ? pagesOfTree(tree) : new Map<PageId, PageInfo>()), [tree, spaceId]);
  const preview = useMemo((): FormulaPreview | null => {
    if (schema === null) {
      return null;
    }
    const page = choosePreviewPage(pages, previewPages, selectedPageId);
    return page === null ? null : { host: buildFormulaHost(schema, { pages, people: people ?? [] }), page: page.id, title: page.title };
  }, [schema, pages, previewPages, selectedPageId, people]);

  if (schema === null) {
    return null;
  }

  async function save(stored: string): Promise<void> {
    const config = { kind: "formula", expression: stored } as const;
    const property =
      def === undefined ? await createProperty(spaceId, name ?? "", "formula", config) : await setPropertyConfig(spaceId, def.id, config);
    onSaved(property);
  }

  return (
    <FormulaEditor
      name={def?.name ?? (name ?? "").trim()}
      schema={schema}
      selfId={def?.id}
      initial={def === undefined ? "" : (formulaExpressionOf(def) ?? "")}
      preview={preview}
      titleLabel={t("pageList.sort.title")}
      onSave={save}
      onCancel={onClose}
      returnFocus={returnRef}
    />
  );
}
