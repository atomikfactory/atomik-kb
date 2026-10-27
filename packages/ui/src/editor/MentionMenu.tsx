// The popup of the `@` menu: the shared suggestion menu with the days and the calendar drawn as a
// glyph, a label and the date they stand for, the people of the space with a round mark of their initial and their name
// ("you" for this account), and the pages as the `[[` picker lists them (the page's icon and its title), under "Dates",
// "People" and "Pages" headers while nothing is typed.

import { initialOf } from "../comments";
import { CalendarIcon } from "../components/icons";
import { PageIcon } from "../components/PageIcon";
import { t } from "../i18n";
import { addDays, mediumDate, todayIso } from "./dates";
import { mentionItemKey, mentionSection } from "./mention-menu";
import type { MentionChoice } from "./mention-menu";
import { SuggestionMenu } from "./suggestion-menu";
import type { SuggestionMenuStore } from "./suggestion-menu";

export interface MentionMenuViewProps {
  store: SuggestionMenuStore<MentionChoice>;
}

/** Draws the `@` menu of `store` next to the cursor. */
export function MentionMenuView({ store }: MentionMenuViewProps) {
  return (
    <SuggestionMenu
      store={store}
      className="slash-menu mention-menu"
      label={t("mention.label")}
      emptyText={t("mention.empty")}
      itemKey={mentionItemKey}
      sections={{
        of: mentionSection,
        label: (section) =>
          t(section === "pages" ? "mention.section.pages" : section === "people" ? "mention.section.people" : "mention.section.dates"),
      }}
      renderItem={(choice) => {
        if (choice.kind === "page") {
          return (
            <>
              <PageIcon icon={choice.page.icon} className="slash-item-icon" />
              <span className="slash-item-text">
                <span className="suggestion-title">{choice.page.title === "" ? t("page.untitled") : choice.page.title}</span>
              </span>
            </>
          );
        }
        if (choice.kind === "me") {
          return (
            <>
              <span className="slash-item-icon person-mark" aria-hidden="true">
                {initialOf(choice.person.name)}
              </span>
              <span className="slash-item-text">
                <span className="suggestion-title">{t("template.mention.me")}</span>
                <span className="suggestion-hint">{t("template.mention.meHint")}</span>
              </span>
            </>
          );
        }
        if (choice.kind === "person") {
          return (
            <>
              <span className="slash-item-icon person-mark" aria-hidden="true">
                {initialOf(choice.person.name)}
              </span>
              <span className="slash-item-text">
                <span className="suggestion-title">{choice.person.name}</span>
                {choice.person.isMe && <span className="suggestion-hint">{t("mention.person.you")}</span>}
              </span>
            </>
          );
        }
        const today = todayIso();
        const title =
          choice.kind === "today" ? t("date.today") : choice.kind === "tomorrow" ? t("date.tomorrow") : t("mention.pick");
        const hint =
          choice.kind === "today"
            ? mediumDate(today)
            : choice.kind === "tomorrow"
              ? mediumDate(addDays(today, 1))
              : t("mention.pick.hint");
        return (
          <>
            <span className="slash-item-icon" aria-hidden="true">
              <CalendarIcon />
            </span>
            <span className="slash-item-text">
              <span className="suggestion-title">{title}</span>
              <span className="suggestion-hint">{hint}</span>
            </span>
          </>
        );
      }}
    />
  );
}
