// The avatars of presence (Milestone 13, slice 3; ADR-0050): a circle of initials in the member's colour with their name as a
// tooltip, for who is on the open page (its header) and who is in a space (the space switcher). You are never shown. A member with
// several devices is one avatar; one whose devices are all idle is dimmed. The row is made of spans, so that it can sit inside the
// space switcher's buttons.

import type { PageId, SpaceId } from "@atomik-kb/core-client";

import { usePagePeople, useSpacePeople } from "../presence-hooks";
import { initialsOf } from "../presence";
import type { PresencePerson } from "../presence";
import { t, tp } from "../i18n";

/** The most avatars drawn; the others are counted in a last one ("+2"). */
const MAX_AVATARS = 4;

function nameOf(person: PresencePerson): string {
  return person.idle ? t("presence.person.idle", { name: person.name }) : person.name;
}

/**
 * A row of avatars. Draws nothing for nobody. `label` names the group for people who cannot see it (and is its tooltip's owner).
 */
export function PresenceAvatars({
  people,
  label,
  max = MAX_AVATARS,
  className,
}: {
  people: readonly PresencePerson[];
  label: string;
  max?: number;
  className?: string;
}) {
  if (people.length === 0) {
    return null;
  }
  // "+N" takes the place of the last avatar it would not fit, so that the row is never longer than `max`.
  const overflow = people.length > max;
  const shown = overflow ? people.slice(0, max - 1) : people;
  const hidden = people.slice(shown.length);
  return (
    <span className={`presence-avatars${className === undefined ? "" : ` ${className}`}`} role="list" aria-label={label}>
      {shown.map((person) => (
        <span
          role="listitem"
          key={person.accountId}
          className={`presence-avatar${person.idle ? " is-idle" : ""}`}
          style={{ backgroundColor: person.colour }}
          title={nameOf(person)}
        >
          <span aria-hidden="true">{initialsOf(person.name)}</span>
          <span className="visually-hidden">{nameOf(person)}</span>
        </span>
      ))}
      {overflow && (
        <span role="listitem" className="presence-avatar presence-avatar-more" title={hidden.map(nameOf).join(", ")}>
          <span aria-hidden="true">+{hidden.length}</span>
          <span className="visually-hidden">{`${tp("presence.more", hidden.length)}: ${hidden.map(nameOf).join(", ")}`}</span>
        </span>
      )}
    </span>
  );
}

/** Who is on a page of a shared space now, for its header. */
export function PageAvatars({ spaceId, pageId }: { spaceId: SpaceId; pageId: PageId }) {
  const people = usePagePeople(spaceId, pageId);
  return <PresenceAvatars people={people} label={t("presence.page.label")} className="presence-page" />;
}

/** Who is in a shared space now, for the space switcher (the button of the space in use and the rows of the list). */
export function SpaceAvatars({ spaceId, max = 3 }: { spaceId: SpaceId; max?: number }) {
  const people = useSpacePeople(spaceId);
  return <PresenceAvatars people={people} label={t("presence.space.label")} max={max} className="presence-space" />;
}
