// The popup of the `:` emoji picker: the shared suggestion menu with each emoji drawn as its
// glyph and its shortcode.

import { t } from "../i18n";
import type { EmojiChoice } from "./emoji-data";
import { SuggestionMenu } from "./suggestion-menu";
import type { SuggestionMenuStore } from "./suggestion-menu";

export interface EmojiMenuProps {
  store: SuggestionMenuStore<EmojiChoice>;
}

/** Draws the emoji picker of `store` next to the cursor. */
export function EmojiMenu({ store }: EmojiMenuProps) {
  return (
    <SuggestionMenu
      store={store}
      className="emoji-menu"
      label={t("emoji.label")}
      // No row for "nothing found": a colon in prose (`:xyz`) should not scold, the menu just closes.
      emptyText=""
      itemKey={(choice) => choice.emoji}
      renderItem={(choice) => (
        <>
          <span className="suggestion-icon emoji-glyph" role="img" aria-label={choice.name}>
            {choice.emoji}
          </span>
          <span className="suggestion-title">{`:${choice.code}:`}</span>
        </>
      )}
    />
  );
}
