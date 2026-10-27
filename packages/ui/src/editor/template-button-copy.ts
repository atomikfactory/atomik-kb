// What a click on a template button does (ADR-0056, `docs/architecture/templates.md` section 9): a copy of the button's content
// inserted right below it (or above it) as one change of the page, with fresh block ids and the relative mentions filled in
// (see `template-button.ts` for the node and the whole feature).

import type { PersonInfo } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { Fragment } from "@tiptap/pm/model";
import type { Node as PmNode } from "@tiptap/pm/model";
import { Selection } from "@tiptap/pm/state";
import type { EditorState, Transaction } from "@tiptap/pm/state";

import { withoutIds } from "./blocks";
import { todayIso } from "./dates";
import { personLabel } from "./person-mention";
import { dateRelativeOf, personRelativeOf, positionOf, TEMPLATE_BUTTON_NODE } from "./template-button";

/** The moment a click happens at: the local day and the local time (`HH:mm`) of this device. */
export interface TemplateMoment {
  date: string;
  time: string;
}

/** The local day and time of `now`. */
export function momentOf(now: Date = new Date()): TemplateMoment {
  const two = (value: number): string => String(value).padStart(2, "0");
  return { date: todayIso(now), time: `${two(now.getHours())}:${two(now.getMinutes())}` };
}

/** Who clicks: what a `me` mention is filled in with. */
export interface TemplateMe {
  accountId: string;
  /** The display name now. */
  name: string;
}

/** This account among the people of a space (`isMe`); `undefined` when the list has none. */
export function meOf(people: readonly PersonInfo[] | undefined): TemplateMe | undefined {
  const mine = people?.find((person) => person.isMe);
  return mine === undefined ? undefined : { accountId: mine.accountId, name: mine.name };
}

/** Whether `node` or anything in it is a person mention to be filled in with `me`. */
export function hasRelativePerson(node: PmNode): boolean {
  let found = false;
  node.descendants((child) => {
    if (found) {
      return false;
    }
    if (child.type.name === "personMention" && personRelativeOf(child.attrs.relative) !== null) {
      found = true;
    }
    return !found;
  });
  return found;
}

/**
 * `node` with its relative mentions filled in: a date mention of `today` takes the day, one of `now` the day and the time, a
 * person mention of `me` the account and the name of `me`; each loses `relative`. A person mention stays as it is when `me` is
 * unknown (nothing is made up). Nodes with nothing to fill in are the same objects.
 */
export function fillRelative(node: PmNode, moment: TemplateMoment, me: TemplateMe | undefined): PmNode {
  if (node.isText) {
    return node;
  }
  if (node.type.name === "dateMention") {
    const relative = dateRelativeOf(node.attrs.relative);
    if (relative === null) {
      return node;
    }
    const written: unknown = node.attrs.time;
    const attrs = { ...node.attrs, date: moment.date, time: relative === "now" ? moment.time : written, relative: null };
    return node.type.create(attrs, null, node.marks);
  }
  if (node.type.name === "personMention") {
    if (personRelativeOf(node.attrs.relative) === null || me === undefined) {
      return node;
    }
    const label = personLabel(me.name);
    const written: unknown = node.attrs.label;
    const attrs = { ...node.attrs, accountId: me.accountId, label: label === "" ? written : label, relative: null };
    return node.type.create(attrs, null, node.marks);
  }
  const children: PmNode[] = [];
  let changed = false;
  node.forEach((child) => {
    const filled = fillRelative(child, moment, me);
    changed ||= filled !== child;
    children.push(filled);
  });
  return changed ? node.type.create(node.attrs, Fragment.fromArray(children), node.marks) : node;
}

/** What {@link insertTemplateCopy} needs to know about the click. */
export interface TemplateCopyOptions {
  moment: TemplateMoment;
  me: TemplateMe | undefined;
}

/**
 * The change that a click on the button at `pos` makes: a copy of its content (new block ids, relative mentions filled in)
 * inserted right below the button, or right above it when its `position` says so, the caret at the end of the copy. One
 * transaction, so one undo step. `null` when `pos` is no button or its content holds nothing.
 */
export function insertTemplateCopy(state: EditorState, pos: number, options: TemplateCopyOptions): Transaction | null {
  const button = state.doc.nodeAt(pos);
  if (button?.type.name !== TEMPLATE_BUTTON_NODE) {
    return null;
  }
  const copies: PmNode[] = [];
  button.forEach((child) => {
    copies.push(fillRelative(withoutIds(child), options.moment, options.me));
  });
  if (copies.length === 0) {
    return null;
  }
  const at = positionOf(button.attrs.position) === "above" ? pos : pos + button.nodeSize;
  const content = Fragment.fromArray(copies);
  const tr = state.tr.insert(at, content);
  tr.setSelection(Selection.near(tr.doc.resolve(Math.min(at + content.size, tr.doc.content.size)), -1));
  return tr.scrollIntoView();
}

/** Clicks the button at `pos` in `editor`: inserts the copy (see {@link insertTemplateCopy}); `false` when nothing was inserted. */
export function clickTemplateButton(editor: Editor, pos: number, me: TemplateMe | undefined, now: Date = new Date()): boolean {
  const tr = insertTemplateCopy(editor.state, pos, { moment: momentOf(now), me });
  if (tr === null) {
    return false;
  }
  editor.view.dispatch(tr);
  editor.view.focus();
  return true;
}
