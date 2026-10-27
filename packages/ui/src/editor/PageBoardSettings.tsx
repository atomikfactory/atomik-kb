// The board's section of the view settings menu (`layoutDef("board").Settings`): the property the cards are grouped by
// (`groupBy`) and, once there is one, which of its columns are shown (`hiddenGroups`; the board's own header has no
// column menu for that). Both write the same patch of the view as the controls that used to sit in the toolbar.

import { t } from "../i18n";
import { boardColumns, isGroupable, resolveGroup } from "./page-board-model";
import type { PageListLayoutSettingsProps } from "./page-list-layout-types";
import { PageListField } from "./PageListField";

const NO_KEYS: readonly string[] = [];

/** The board's settings: the group-by select and the checkboxes of the columns. */
export function PageBoardSettings({ view, schema, onUpdateView }: PageListLayoutSettingsProps) {
  const hiddenGroups = view.hiddenGroups ?? NO_KEYS;
  const group = resolveGroup(view.groupBy ?? null, schema);
  return (
    <>
      <PageListField label={t("pageBoard.groupBy")}>
        <select
          className="page-list-select"
          value={group.kind === "ok" ? group.def.id : ""}
          onChange={(event) => {
            if (event.target.value !== "") {
              onUpdateView({ groupBy: event.target.value });
            }
          }}
        >
          <option value="" disabled>
            {t("pageBoard.groupBy.choose")}
          </option>
          {schema.filter(isGroupable).map((def) => (
            <option key={def.id} value={def.id}>
              {def.name}
            </option>
          ))}
        </select>
      </PageListField>
      {group.kind === "ok" && (
        <div role="group" aria-label={t("pageBoard.groups.label")} className="page-list-field">
          <span className="page-list-field-label">
            {t("pageBoard.groups")}
            {hiddenGroups.length > 0 && <span className="page-list-count">{` ${String(hiddenGroups.length)}`}</span>}
          </span>
          <div className="page-list-settings-list">
            {boardColumns(group.def).map((column) => (
              <label key={column.key} className="page-list-prop">
                <input
                  type="checkbox"
                  checked={!hiddenGroups.includes(column.key)}
                  onChange={(event) => {
                    const others = hiddenGroups.filter((key) => key !== column.key);
                    onUpdateView({ hiddenGroups: event.target.checked ? others : [...others, column.key] });
                  }}
                />
                <span>{column.option?.name ?? t("pageBoard.none", { name: group.def.name })}</span>
              </label>
            ))}
          </div>
        </div>
      )}
    </>
  );
}
