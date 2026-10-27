// The chip of a person mention (ADR-0049): `@` and the member's current name from the space's people, so a rename shows; an account
// the space no longer lists reads "Former member (label)" with the name the mention was made with; while the people are
// not read yet, the label. The member who is this account has its own colour. Read only: a click does nothing.

import { NodeViewWrapper } from "@tiptap/react";
import type { ReactNodeViewProps } from "@tiptap/react";

import { isFormerMember } from "../comments";
import { usePeople, usePersonNames } from "../comments-hooks";
import { FilledInIcon } from "../components/template-icons";
import { t } from "../i18n";
import type { PersonMentionOptions } from "./person-mention";

export function PersonMentionView({ node, extension }: ReactNodeViewProps) {
  const options = extension.options as PersonMentionOptions;
  const people = usePeople(options.spaceId);
  const accountId = typeof node.attrs.accountId === "string" ? node.attrs.accountId : "";
  const label = typeof node.attrs.label === "string" ? node.attrs.label : "";
  // The last name the space knew a person who left by, else the label the mention was made with.
  const name = usePersonNames(options.spaceId, [accountId])(accountId, label);
  const former = isFormerMember(people, accountId);
  const mine = people?.find((person) => person.accountId === accountId)?.isMe === true;
  // A relative mention (ADR-0056) reads "@Me" with a mark: the person who makes a page from its template is who it names.
  const relative = node.attrs.relative === "me";
  const classes = ["person-mention", former && !relative ? "is-former" : "", mine && !relative ? "is-me" : "", relative ? "is-relative" : ""]
    .filter((name) => name !== "")
    .join(" ");
  return (
    <NodeViewWrapper as="span" className="person-mention-wrapper">
      <span
        className={classes}
        data-account-id={accountId}
        title={relative ? t("template.relative.mark") : former ? t("comments.formerMember.title") : undefined}
      >
        @{relative ? t("template.relative.me") : name}
        {relative && (
          <span className="person-mention-relative" aria-hidden="true">
            <FilledInIcon />
          </span>
        )}
      </span>
    </NodeViewWrapper>
  );
}
