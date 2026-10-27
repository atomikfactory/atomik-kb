// The editor of a `place` property (ADR-0026, amendment of 2026-10-08): the name (an address or a
// label), the latitude and longitude if they are known, and an "Open in map" action. Nothing is
// looked up and no map is drawn: the action opens OpenStreetMap in the person's browser through
// the shell's external-link opener, and only when they press it.
//
// The three fields are one value: it is saved when the focus leaves the editor or on Enter, as
// one `pagePropertySet`. A draft that breaks a rule is not sent: the reason shows under the fields.
// Escape puts the fields back as they were. Clearing all three clears the value.

import { propertyValuesEqual } from "@atomik-kb/core-client";
import { useRef, useState } from "react";
import type { FocusEvent, KeyboardEvent } from "react";

import { t } from "../i18n";
import { parsePlaceDraft, placeDraftOf, placeMapUrl } from "../property-format";
import { useAppStore } from "../store-context";
import { ErrorLine, useCommit } from "./property-editor-parts";
import type { PropertyEditorProps } from "./property-editor-parts";
import { OpenIcon } from "./property-icons";

export function PlaceEditor({ def, value, readOnly, autoFocus, commit }: PropertyEditorProps) {
  const openExternal = useAppStore((state) => state.openExternal);
  const { run, error, clearError } = useCommit(commit);
  const stored = placeDraftOf(value);
  const storedKey = `${stored.name}\n${stored.lat}\n${stored.lon}`;
  const [draft, setDraft] = useState(stored);
  // What the page holds changed (a save, or an edit from elsewhere): the fields follow it.
  const [seen, setSeen] = useState(storedKey);
  const [attempted, setAttempted] = useState(false);
  const reverted = useRef(false);
  if (seen !== storedKey) {
    setSeen(storedKey);
    setDraft(stored);
    setAttempted(false);
  }

  const dirty = draft.name !== stored.name || draft.lat !== stored.lat || draft.lon !== stored.lon;
  const parsed = parsePlaceDraft(draft);
  const problem = dirty && !parsed.ok && attempted ? parsed.problem : null;

  async function save(): Promise<void> {
    if (!dirty) {
      return;
    }
    const result = parsePlaceDraft(draft);
    if (!result.ok) {
      setAttempted(true);
      return;
    }
    const unchanged =
      result.value === null ? value === undefined : value !== undefined && propertyValuesEqual(result.value, value);
    if (unchanged) {
      setDraft(stored);
      return;
    }
    await run(result.value);
  }

  function onKeyDown(event: KeyboardEvent<HTMLInputElement>): void {
    if (event.nativeEvent.isComposing) {
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      reverted.current = true;
      setDraft(stored);
      setAttempted(false);
      clearError();
      event.currentTarget.blur();
    } else if (event.key === "Enter") {
      event.preventDefault();
      setAttempted(true);
      void save();
    }
  }

  function onBlur(event: FocusEvent<HTMLDivElement>): void {
    // Moving between the fields of the editor is not leaving it.
    if (event.relatedTarget instanceof Node && event.currentTarget.contains(event.relatedTarget)) {
      return;
    }
    if (reverted.current) {
      reverted.current = false;
      return;
    }
    setAttempted(true);
    void save();
  }

  const place = value?.kind === "place" ? value.value : null;
  const mapButton =
    place === null ? null : (
      <button
        type="button"
        className="icon-button"
        aria-label={t("properties.place.openMap", { name: place.name })}
        title={t("properties.place.openMap", { name: place.name })}
        onClick={() => {
          void openExternal(placeMapUrl(place));
        }}
      >
        <OpenIcon />
      </button>
    );

  if (readOnly) {
    return (
      <div className="property-editor">
        <span className="properties-text">{place === null ? "" : place.name}</span>
        {place?.lat !== undefined && place.lon !== undefined && (
          <span className="properties-text muted">{`${String(place.lat)}, ${String(place.lon)}`}</span>
        )}
        {mapButton}
      </div>
    );
  }

  const change = (field: "name" | "lat" | "lon") => (event: { target: { value: string } }) => {
    setDraft((current) => ({ ...current, [field]: event.target.value }));
    setAttempted(false);
    clearError();
  };

  return (
    <div className="property-editor place-editor" onBlur={onBlur}>
      <input
        className="property-input place-name"
        type="text"
        spellCheck={false}
        autoComplete="off"
        autoFocus={autoFocus}
        aria-label={def.name}
        aria-invalid={problem !== null}
        placeholder={t("properties.placeholder.place")}
        value={draft.name}
        onChange={change("name")}
        onKeyDown={onKeyDown}
      />
      <input
        className="property-input place-coordinate"
        type="text"
        inputMode="decimal"
        spellCheck={false}
        autoComplete="off"
        aria-label={t("properties.place.lat")}
        aria-invalid={problem !== null}
        placeholder={t("properties.place.lat")}
        value={draft.lat}
        onChange={change("lat")}
        onKeyDown={onKeyDown}
      />
      <input
        className="property-input place-coordinate"
        type="text"
        inputMode="decimal"
        spellCheck={false}
        autoComplete="off"
        aria-label={t("properties.place.lon")}
        aria-invalid={problem !== null}
        placeholder={t("properties.place.lon")}
        value={draft.lon}
        onChange={change("lon")}
        onKeyDown={onKeyDown}
      />
      {mapButton}
      <ErrorLine message={problem ?? error} />
    </div>
  );
}
