// Pieces both sides of a pairing show (ADR-0022): the code in three groups of four and the
// six digits the user compares, with the warning that goes with them.

import { useEffect, useId, useRef } from "react";

import { t } from "../i18n";
import { digitGroups, pairingCodeGroups } from "../pairing";

/** The pairing code (or the code of an invitation to a space, with its own `label`), large, in three groups of four. */
export function PairingCode({ code, label }: { code: string; label?: string }) {
  return (
    <p className="pair-code" role="group" aria-label={label ?? t("join.codeLabel")}>
      {pairingCodeGroups(code).map((group, index) => (
        <span className="pair-code-group" key={index}>
          {group}
        </span>
      ))}
    </p>
  );
}

/**
 * The six digits, large, in two groups of three, and the plain-words warning under them (`warningText` words it for
 * people instead of devices, as the invitations to a space do).
 */
export function PairingDigits({
  sas,
  warning = true,
  warningText,
}: {
  sas: string;
  warning?: boolean;
  warningText?: string;
}) {
  const warningId = useId();
  return (
    <>
      <p
        className="pair-digits"
        role="group"
        aria-label={t("pair.digitsLabel")}
        aria-describedby={warning ? warningId : undefined}
      >
        {digitGroups(sas).map((group, index) => (
          <span className="pair-digits-group" key={index}>
            {group}
          </span>
        ))}
      </p>
      {warning && (
        <p className="note pair-warning" id={warningId}>
          {warningText ?? t("pair.warning")}
        </p>
      )}
    </>
  );
}

/**
 * The steps of a session replace the form that had focus: the container takes it when it
 * appears (keys such as Escape keep reaching the dialog, a screen reader starts here).
 */
export function useFocusOnMount<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  useEffect(() => {
    ref.current?.focus();
  }, []);
  return ref;
}

/** A line with a spinner, for the steps that only wait. */
export function PairingProgress({ text }: { text: string }) {
  return (
    <p className="pair-progress" role="status">
      <span className="pair-spinner" aria-hidden="true" />
      {text}
    </p>
  );
}
