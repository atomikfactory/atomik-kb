// The "Remind" choice of the date picker in a shared space (Milestone 13, slice 2; ADR-0047): once a mention has a reminder, the picker
// says who is reminded: the Owners of the space (nobody named, the default), this account, or another member. The choice is the
// mention's `reminderFor` and nothing else changes; a personal space has no such choice, and a mention without a reminder has no one to
// remind. A person who left stays listed as "Former member (name)".

import type { JsonValue, SeedSpace } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import type { Editor } from "@tiptap/core";
import { describe, expect, it } from "vitest";

import { t } from "../i18n";
import { openWorkPage, renderComments } from "../test/comments";
import { pid } from "../test/editor";
import { findEditor, idsOf } from "../test/page";
import { clickPage, openSpace } from "../test/sharing";

const NODE = pid(11);

/** A paragraph with a date mention: with a reminder 15 minutes before, or none. */
function content(reminder: boolean): JsonValue {
  return {
    type: "doc",
    content: [
      {
        type: "paragraph",
        attrs: { id: pid(10) },
        content: [
          { type: "text", text: "Meet " },
          { type: "dateMention", attrs: { id: NODE, date: "2026-10-07", time: "14:30", ...(reminder ? { reminder: 15 } : {}) } },
        ],
      },
    ],
  };
}

const SPACES: readonly SeedSpace[] = [
  { name: "Personal", pages: [{ title: "Diary", content: content(true) }] },
  {
    name: "Work",
    pages: [
      { title: "Plan", content: content(true) },
      { title: "Notes", content: content(false) },
    ],
  },
];

async function openPicker(user: Parameters<typeof openWorkPage>[0], editor: Editor): Promise<HTMLElement> {
  const chip = editor.view.dom.querySelector<HTMLElement>(".date-mention");
  if (chip === null) {
    throw new Error("no chip");
  }
  await user.click(chip);
  return screen.findByRole("dialog", { name: t("date.pickerTitle") });
}

/** The `reminderFor` the page holds now (`undefined`: none), read from the core. */
async function storedReminderFor(page: string, client: Awaited<ReturnType<typeof renderComments>>["client"]): Promise<string | undefined> {
  const found = /"reminderFor":"([^"]+)"/u.exec(JSON.stringify((await client.pageContentGet(page)).content));
  return found?.[1];
}

describe("who a reminder is for", () => {
  it("lists the Owners of the space, this account and the other members once a mention has a reminder", async () => {
    const { user } = await renderComments({ role: "owner", memory: { spaces: SPACES } });
    const editor = await openWorkPage(user, "Plan");
    const picker = await openPicker(user, editor);
    const select = await within(picker).findByRole("combobox", { name: t("date.remindWho") });
    expect(select).toHaveValue("owners");
    const options = within(select).getAllByRole("option").map((option) => option.textContent);
    // The Owners (the default) and this account come first; then the other members, by name.
    expect(options.slice(0, 2)).toEqual([t("date.remindWho.owners"), t("date.remindWho.me")]);
    expect(options.slice(2)).toEqual(expect.arrayContaining(["Bob", "Carol", "Dan"]));
    expect(options).not.toContain("Maya");
  });

  it("sets reminderFor to this account, to another member, and takes it out for the Owners", async () => {
    const { user, client, memory } = await renderComments({ role: "owner", memory: { spaces: SPACES } });
    const ids = await idsOf(memory);
    const plan = ids.page("Plan");
    const people = await client.peopleMentionable(ids.space("Work"));
    const me = people.find((person) => person.isMe)?.accountId;
    const bob = people.find((person) => person.name === "Bob")?.accountId;
    const editor = await openWorkPage(user, "Plan");
    const picker = await openPicker(user, editor);
    const select = await within(picker).findByRole("combobox", { name: t("date.remindWho") });

    await user.selectOptions(select, t("date.remindWho.me"));
    await waitFor(async () => {
      expect(await storedReminderFor(plan, client)).toBe(me);
    });
    await user.selectOptions(select, "Bob");
    await waitFor(async () => {
      expect(await storedReminderFor(plan, client)).toBe(bob);
    });
    expect(select).toHaveValue(bob);
    // Only reminderFor changed: the reminder is the one it was.
    expect(JSON.stringify((await client.pageContentGet(plan)).content)).toContain('"reminder":15');
    await user.selectOptions(select, t("date.remindWho.owners"));
    await waitFor(async () => {
      expect(await storedReminderFor(plan, client)).toBeUndefined();
    });
    expect(JSON.stringify((await client.pageContentGet(plan)).content)).toContain('"reminder":15');
  });

  it("takes reminderFor out together with the reminder, and has no choice for a mention without a reminder", async () => {
    const { user, client, memory } = await renderComments({ role: "owner", memory: { spaces: SPACES } });
    const ids = await idsOf(memory);
    const plan = ids.page("Plan");
    const editor = await openWorkPage(user, "Plan");
    const picker = await openPicker(user, editor);
    await user.selectOptions(await within(picker).findByRole("combobox", { name: t("date.remindWho") }), t("date.remindWho.me"));
    await waitFor(async () => {
      expect(await storedReminderFor(plan, client)).toBeDefined();
    });
    await user.selectOptions(within(picker).getByRole("combobox", { name: t("date.reminder") }), t("date.reminder.none"));
    await waitFor(async () => {
      expect(await storedReminderFor(plan, client)).toBeUndefined();
    });
    expect(JSON.stringify((await client.pageContentGet(plan)).content)).not.toContain('"reminder"');
    expect(within(picker).queryByRole("combobox", { name: t("date.remindWho") })).not.toBeInTheDocument();
  });

  it("is not offered for a mention that has no reminder yet, and appears when one is set", async () => {
    const { user } = await renderComments({ role: "owner", memory: { spaces: SPACES } });
    await openSpace(user, "Work");
    await clickPage(user, "Notes");
    const picker = await openPicker(user, await findEditor());
    expect(within(picker).queryByRole("combobox", { name: t("date.remindWho") })).not.toBeInTheDocument();
    await user.selectOptions(within(picker).getByRole("combobox", { name: t("date.reminder") }), t("date.reminder.m15"));
    expect(await within(picker).findByRole("combobox", { name: t("date.remindWho") })).toBeInTheDocument();
  });

  it("is not offered in a personal space", async () => {
    const { user } = await renderComments({ role: "owner", memory: { spaces: SPACES } });
    await openSpace(user, "Personal");
    await clickPage(user, "Diary");
    const picker = await openPicker(user, await findEditor());
    // The reminder is there; who it is for is not a question of a space of one.
    expect(within(picker).getByRole("combobox", { name: t("date.reminder") })).toBeInTheDocument();
    expect(within(picker).queryByRole("combobox", { name: t("date.remindWho") })).not.toBeInTheDocument();
  });

  it("keeps the account of a person who left listed as a former member with the last name", async () => {
    const { user, client, memory } = await renderComments({ role: "owner", memory: { spaces: SPACES } });
    const ids = await idsOf(memory);
    const bob = (await client.peopleMentionable(ids.space("Work"))).find((person) => person.name === "Bob")?.accountId ?? "";
    const editor = await openWorkPage(user, "Plan");
    const picker = await openPicker(user, editor);
    const select = await within(picker).findByRole("combobox", { name: t("date.remindWho") });
    await user.selectOptions(select, "Bob");
    await waitFor(async () => {
      expect(await storedReminderFor(ids.page("Plan"), client)).toBe(bob);
    });
    act(() => {
      memory.receiveSharing({ type: "removed", spaceId: ids.space("Work"), accountId: bob });
    });
    const former = t("comments.formerMember.label", { label: "Bob" });
    await waitFor(() => {
      expect(within(select).getByRole("option", { name: former })).toBeInTheDocument();
    });
    expect(select).toHaveValue(bob);
    expect(within(select).queryByRole("option", { name: "Bob" })).not.toBeInTheDocument();
  });
});
