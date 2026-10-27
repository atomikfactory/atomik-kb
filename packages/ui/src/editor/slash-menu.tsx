// The popup of the `/` menu: the shared suggestion menu with the slash entries drawn as a
// glyph, a label and a hint (or why the entry is unavailable), grouped under section headers.

import type { Editor } from "@tiptap/core";

import { t } from "../i18n";
import { SLASH_SECTION_LABELS } from "./slash-commands";
import type { SlashCommand, SlashSection } from "./slash-commands";
import { SuggestionMenu } from "./suggestion-menu";
import type { SuggestionMenuStore } from "./suggestion-menu";

export interface SlashMenuProps {
  store: SuggestionMenuStore<SlashCommand>;
  /** The editor the entries run in, or `null` before it exists (every entry then shows as usual). */
  editor: Editor | null;
}

/** Draws the `/` menu of `store` next to the cursor. */
export function SlashMenu({ store, editor }: SlashMenuProps) {
  return (
    <SuggestionMenu
      store={store}
      className="slash-menu"
      label={t("slash.label")}
      emptyText={t("slash.empty")}
      itemKey={(command) => command.id}
      isDisabled={(command) => editor !== null && command.unavailable?.(editor) != null}
      sections={{
        of: (command) => command.section,
        label: (section) => t(SLASH_SECTION_LABELS[section as SlashSection]),
      }}
      renderItem={(command) => {
        const unavailable = editor === null ? null : (command.unavailable?.(editor) ?? null);
        const Icon = command.icon;
        return (
          <>
            <span className="slash-item-icon" aria-hidden="true">
              <Icon />
            </span>
            <span className="slash-item-text">
              <span className="suggestion-title">{t(command.label)}</span>
              <span className="suggestion-hint">{t(unavailable ?? command.hint)}</span>
            </span>
          </>
        );
      }}
    />
  );
}
