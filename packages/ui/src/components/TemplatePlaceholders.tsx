// The placeholders of the properties panel (ADR-0056, `docs/architecture/templates.md` section 6). On a page in a template a `date`
// property can say "Today (when created)" or "Now (when created)" and a `person` property "Me (whoever creates)": the page made
// from the template gets the day, the moment or the person who made it. A property with a placeholder shows it as a chip with a
// "filled in" mark and a Clear button instead of its value editor; a property without one gets a small menu button next to its
// editor that offers them.

import type { PropertyDef, TemplatePlaceholder } from "@atomik-kb/core-client";
import { useState } from "react";

import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { anchorBelow } from "./FloatingPanel";
import type { Anchor } from "./FloatingPanel";
import { Menu } from "./Menu";
import { FilledInIcon } from "./template-icons";

/** What each placeholder says. */
export const PLACEHOLDER_LABELS: Readonly<Record<TemplatePlaceholder, MessageKey>> = {
  today: "template.placeholder.today",
  now: "template.placeholder.now",
  me: "template.placeholder.me",
};

/** The placeholders a kind of property takes (the core refuses the others). */
export function placeholdersFor(kind: PropertyDef["kind"]): readonly TemplatePlaceholder[] {
  return kind === "date" ? ["today", "now"] : kind === "person" ? ["me"] : [];
}

/** A property whose value is filled in when a page is made from the template. */
export function PlaceholderValue({
  def,
  placeholder,
  readOnly,
  onClear,
}: {
  def: PropertyDef;
  placeholder: TemplatePlaceholder;
  readOnly: boolean;
  onClear: () => void;
}) {
  return (
    <div className="property-editor template-placeholder" data-placeholder={placeholder}>
      <span className="property-chip template-placeholder-chip">
        <span className="template-placeholder-label">{t(PLACEHOLDER_LABELS[placeholder])}</span>
        <span className="template-filled-mark" title={t("template.placeholder.markTitle")}>
          <FilledInIcon />
          <span>{t("template.placeholder.filledIn")}</span>
        </span>
      </span>
      {!readOnly && (
        <button
          type="button"
          className="link-button template-placeholder-clear"
          aria-label={t("template.placeholder.clear", { name: def.name })}
          onClick={onClear}
        >
          {t("template.placeholder.clearLabel")}
        </button>
      )}
    </div>
  );
}

/** The button next to the editor of a date or person property that offers its placeholders. */
export function PlaceholderMenuButton({
  def,
  onChoose,
}: {
  def: PropertyDef;
  onChoose: (placeholder: TemplatePlaceholder) => void;
}) {
  const [menu, setMenu] = useState<{ anchor: Anchor; trigger: Element } | null>(null);
  const choices = placeholdersFor(def.kind);
  if (choices.length === 0) {
    return null;
  }
  return (
    <>
      <button
        type="button"
        className="icon-button template-placeholder-button"
        aria-haspopup="menu"
        aria-expanded={menu !== null}
        aria-label={t("template.placeholder.menu", { name: def.name })}
        title={t("template.placeholder.menu", { name: def.name })}
        onClick={(event) => {
          const trigger = event.currentTarget;
          setMenu((current) => (current === null ? { anchor: anchorBelow(trigger), trigger } : null));
        }}
      >
        <FilledInIcon />
      </button>
      {menu !== null && (
        <Menu
          label={t("template.placeholder.menuLabel", { name: def.name })}
          items={choices.map((choice) => ({ id: choice, label: t(PLACEHOLDER_LABELS[choice]) }))}
          anchor={menu.anchor}
          anchorElement={menu.trigger}
          onSelect={(id) => {
            const chosen = choices.find((choice) => choice === id);
            if (chosen !== undefined) {
              onChoose(chosen);
            }
          }}
          onClose={() => {
            setMenu(null);
          }}
        />
      )}
    </>
  );
}
