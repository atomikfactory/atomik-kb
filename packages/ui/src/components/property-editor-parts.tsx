// What the editors of the properties panel share (ADR-0026): their props, the commit helper that
// keeps a failure for the editor to show, and the line that shows it.

import { toCoreError } from "@atomik-kb/core-client";
import type { PropertyDef, PropertyOption, PropertyValue, SpaceId } from "@atomik-kb/core-client";
import { useState } from "react";

import { t } from "../i18n";

export interface PropertyEditorProps {
  def: PropertyDef;
  spaceId: SpaceId;
  /** The page's value; `undefined` when it has none. */
  value: PropertyValue | undefined;
  /** Shows the value and changes nothing (a trashed page). */
  readOnly: boolean;
  /** Takes focus when it appears (a property that was just added). */
  autoFocus: boolean;
  /** Saves a value (`null` clears it); rejects with a `CoreError`. */
  commit: (value: PropertyValue | null) => Promise<void>;
  /** Asks to delete an option of the property (the owner confirms). */
  onDeleteOption: (option: PropertyOption) => void;
}

/** Runs a commit and keeps its failure for the editor to show. */
export function useCommit(commit: PropertyEditorProps["commit"]) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  return {
    error,
    busy,
    clearError: () => {
      setError(null);
    },
    /** `true` when the core took the value. */
    run: async (value: PropertyValue | null): Promise<boolean> => {
      setBusy(true);
      setError(null);
      try {
        await commit(value);
        setBusy(false);
        return true;
      } catch (failure) {
        setError(t("properties.error.save", { message: toCoreError(failure).message }));
        setBusy(false);
        return false;
      }
    },
  };
}

/** The message under an editor: a refusal of the core or a value that cannot be sent. */
export function ErrorLine({ message }: { message: string | null }) {
  return message === null ? null : (
    <p className="error-text properties-error" role="alert">
      {message}
    </p>
  );
}
