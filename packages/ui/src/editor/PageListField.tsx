// A labelled control of the view's popovers (the sort, the view settings): the label above, the control under it, the
// label read as the control's name. The layouts' settings sections use it too, so every setting looks the same.

import type { ReactNode } from "react";

export interface PageListFieldProps {
  /** The visible label, which is also the control's accessible name. */
  readonly label: string;
  /** The control: a `select` (class `page-list-select`). */
  readonly children: ReactNode;
}

export function PageListField({ label, children }: PageListFieldProps) {
  return (
    <label className="page-list-field">
      <span className="page-list-field-label">{label}</span>
      {children}
    </label>
  );
}
