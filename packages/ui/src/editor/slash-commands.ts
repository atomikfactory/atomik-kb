// The `/` menu: block types to insert or turn the current block into, images and files (the
// shell's file picker), list, table and board views of pages, a new sub-page (empty, or from a template, ADR-0056) and a link to a page (a card), the
// one-shot sorts of the open page's sub-pages and of the list the caret is in (ADR-0052), and the microphone entries:
// dictation (`/microphone`, ADR-0053) and the recorder (`/record`, ADR-0037). Entries are grouped in
// sections (`SLASH_SECTIONS`), each with a glyph, and found by ranked matching.

import { Extension } from "@tiptap/core";
import type { Editor, Range } from "@tiptap/core";
import { PluginKey } from "@tiptap/pm/state";
import { Suggestion } from "@tiptap/suggestion";
import type { ComponentType } from "react";

import {
  AtIcon,
  BreadcrumbIcon,
  BulletListIcon,
  CalendarIcon,
  CalendarViewIcon,
  CalloutIcon,
  CheckIcon,
  CodeIcon,
  Columns2Icon,
  Columns3Icon,
  DashboardViewIcon,
  DiagramIcon,
  DividerIcon,
  EmojiIcon,
  EquationIcon,
  FileIcon,
  GalleryIcon,
  GalleryViewIcon,
  PdfIcon,
  VideoIcon,
  AudioIcon,
  Heading1Icon,
  Heading2Icon,
  Heading3Icon,
  ImageIcon,
  InlineEquationIcon,
  LinkIcon,
  ListViewIcon,
  BoardViewIcon,
  TableViewIcon,
  NumberedListIcon,
  QuoteIcon,
  SortIcon,
  SubPageIcon,
  TableIcon,
  TemplateButtonIcon,
  TextIcon,
  TocIcon,
  ToggleHeading1Icon,
  ToggleHeading2Icon,
  ToggleHeading3Icon,
  ToggleIcon,
  WhiteboardIcon,
} from "../components/icons";
import { PageFromTemplateIcon } from "../components/template-icons";
import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { editorAiOf, recordingOn, writingOn } from "./ai/EditorAi";
import { RecordIcon, SparkleIcon } from "./ai/icons";
import { RecorderController } from "./ai/recorder";
import { importAttachment, importImageFromUrl, importUnavailable, imageUrlHidden, insertMedia, insertPdf, uploadUnavailable } from "./attachments";
import { MERMAID_LANGUAGE } from "./code-block";
import { columnsUnavailable } from "./columns";
import { takesInlineAtom } from "./date-mention";
import { todayIso } from "./dates";
import { DictateIcon } from "./dictation/icons";
import { dictationOf, dictationOn, dictationRunning, dictationUnavailable } from "./dictation/registry";
import { openEmojiPicker } from "./emoji-picker";
import { innermostListPos, sortListIn } from "./list-sort";
import { insideTemplateButton } from "./template-button";
import { openMentionMenu } from "./mention-menu";
import { openPagePicker } from "./page-picker";
import {
  insertSubPage,
  insertSubPageFromTemplate,
  sortSubpages,
  sortSubpagesUnavailable,
  subPageUnavailable,
  templatePageUnavailable,
} from "./sub-page";
import type { SuggestionMenuStore } from "./suggestion-menu";

/**
 * The groups of the menu, in the order they are listed. `inline` holds what goes inside text
 * (a date, a mention, an emoji, an equation).
 */
export const SLASH_SECTIONS = ["basic", "media", "advanced", "pages", "inline"] as const;

export type SlashSection = (typeof SLASH_SECTIONS)[number];

/** The header of each section (small caps in the menu while nothing is typed). */
export const SLASH_SECTION_LABELS: Readonly<Record<SlashSection, MessageKey>> = {
  basic: "slash.section.basic",
  media: "slash.section.media",
  advanced: "slash.section.advanced",
  pages: "slash.section.pages",
  inline: "slash.section.inline",
};

/**
 * One entry of the menu. To add one, append it to `ENTRIES` below with its `section` (the
 * list is ordered by section, so where it sits in the source does not matter), an `icon`
 * from `components/icons.tsx`, a label and a hint in `en.json` (`slash.<id>`,
 * `slash.<id>.hint`) and the words it should be found by.
 */
export interface SlashCommand {
  id: string;
  section: SlashSection;
  /** A 16 px glyph that follows the text color (decorative: the label names the entry). */
  icon: ComponentType;
  label: MessageKey;
  hint: MessageKey;
  /** Extra words the menu matches (lowercase). */
  keywords: readonly string[];
  /**
   * Why the command cannot run in this editor (shown in place of the hint), or `null` when
   * it can. Commands without it always run.
   */
  unavailable?: (editor: Editor) => MessageKey | null;
  /**
   * Whether the entry is left out of the menu in this editor (nothing the person could do would
   * make it work here, so there is no reason to show it). Commands without it are always listed.
   */
  hidden?: (editor: Editor) => boolean;
  run: (editor: Editor, range: Range) => void;
}

/** Deletes the typed `/query`, then runs `then` on the block where it was. */
function inBlock(editor: Editor, range: Range) {
  return editor.chain().focus().deleteRange(range);
}

/**
 * Inserts a table of 3 x 3 cells with a header row where the `/query` was, the caret in its
 * first cell. A table that ends the page (or the cell it was typed in) gets an empty
 * paragraph after it, so there is always a block to type into below it.
 */
function insertTable(editor: Editor, range: Range): void {
  inBlock(editor, range).insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run();
  const { $from } = editor.state.selection;
  for (let depth = $from.depth; depth > 0; depth -= 1) {
    const table = $from.node(depth);
    if (table.type.name === "table") {
      if ($from.node(depth - 1).lastChild === table) {
        editor.commands.insertContentAt($from.after(depth), { type: "paragraph" }, { updateSelection: false });
      }
      return;
    }
  }
}

/**
 * Sorts the list the caret is in, A to Z (ADR-0052), in the same transaction that deletes the typed `/sort...`: one
 * undo step brings back both the order and the typed text, as for every other command of the menu. A list that is
 * in order already only loses the typed text.
 */
function sortList(editor: Editor, range: Range): void {
  const tr = editor.state.tr.delete(range.from, range.to);
  const listPos = innermostListPos(tr.doc, tr.mapping.map(range.from));
  if (listPos !== null) {
    sortListIn(tr, listPos);
  }
  editor.view.dispatch(tr);
  editor.view.focus();
}

/**
 * Why "Record audio" cannot run in `editor` now, `null` when it can: dictation holds the microphone (they exclude each
 * other, ADR-0053), the page cannot store the recording, or the browser cannot record.
 */
function recordUnavailable(editor: Editor): MessageKey | null {
  if (dictationRunning(editor)) {
    return "slash.record.dictating";
  }
  return uploadUnavailable(editor) ?? (RecorderController.supported() ? null : "recorder.unsupported");
}

/** What `/diagram` starts a diagram with: a two-node flowchart to edit. */
export const DIAGRAM_STARTER = "flowchart TD\n  A[Start] --> B[End]";

/**
 * Turns the block where the `/query` was into a diagram: a code block in `mermaid` holding
 * a starter flowchart (the preview draws it), the caret at the end of the source.
 */
function insertDiagram(editor: Editor, range: Range): void {
  inBlock(editor, range)
    .setCodeBlock({ language: MERMAID_LANGUAGE })
    .insertContent({ type: "text", text: DIAGRAM_STARTER })
    .run();
}

const ENTRIES: readonly SlashCommand[] = [
  // ----- Basic: the blocks text is made of
  {
    id: "text",
    section: "basic",
    icon: TextIcon,
    label: "slash.text",
    hint: "slash.text.hint",
    keywords: ["paragraph", "plain"],
    run: (editor, range) => inBlock(editor, range).setParagraph().run(),
  },
  {
    id: "heading1",
    section: "basic",
    icon: Heading1Icon,
    label: "slash.heading1",
    hint: "slash.heading1.hint",
    keywords: ["h1", "title"],
    run: (editor, range) => inBlock(editor, range).setHeading({ level: 1 }).run(),
  },
  {
    id: "heading2",
    section: "basic",
    icon: Heading2Icon,
    label: "slash.heading2",
    hint: "slash.heading2.hint",
    keywords: ["h2", "subtitle"],
    run: (editor, range) => inBlock(editor, range).setHeading({ level: 2 }).run(),
  },
  {
    id: "heading3",
    section: "basic",
    icon: Heading3Icon,
    label: "slash.heading3",
    hint: "slash.heading3.hint",
    keywords: ["h3"],
    run: (editor, range) => inBlock(editor, range).setHeading({ level: 3 }).run(),
  },
  {
    id: "bulletList",
    section: "basic",
    icon: BulletListIcon,
    label: "slash.bulletList",
    hint: "slash.bulletList.hint",
    keywords: ["ul", "list", "bullet", "unordered"],
    run: (editor, range) => inBlock(editor, range).toggleBulletList().run(),
  },
  {
    id: "orderedList",
    section: "basic",
    icon: NumberedListIcon,
    label: "slash.orderedList",
    hint: "slash.orderedList.hint",
    keywords: ["ol", "list", "numbered", "ordered"],
    run: (editor, range) => inBlock(editor, range).toggleOrderedList().run(),
  },
  {
    id: "taskList",
    section: "basic",
    icon: CheckIcon,
    label: "slash.taskList",
    hint: "slash.taskList.hint",
    keywords: ["todo", "task", "checkbox", "check"],
    run: (editor, range) => inBlock(editor, range).toggleTaskList().run(),
  },
  {
    id: "toggle",
    section: "basic",
    icon: ToggleIcon,
    label: "slash.toggle",
    hint: "slash.toggle.hint",
    keywords: ["details", "collapse", "fold"],
    run: (editor, range) => inBlock(editor, range).setDetails().run(),
  },
  // Toggle headings (ADR-0040): a toggle whose summary is a heading; found by toggle, heading and h1 to h3.
  {
    id: "toggleHeading1",
    section: "basic",
    icon: ToggleHeading1Icon,
    label: "slash.toggleHeading1",
    hint: "slash.toggleHeading1.hint",
    keywords: ["toggle", "heading", "h1", "collapse", "fold"],
    run: (editor, range) => inBlock(editor, range).setToggleHeading(1).run(),
  },
  {
    id: "toggleHeading2",
    section: "basic",
    icon: ToggleHeading2Icon,
    label: "slash.toggleHeading2",
    hint: "slash.toggleHeading2.hint",
    keywords: ["toggle", "heading", "h2", "collapse", "fold"],
    run: (editor, range) => inBlock(editor, range).setToggleHeading(2).run(),
  },
  {
    id: "toggleHeading3",
    section: "basic",
    icon: ToggleHeading3Icon,
    label: "slash.toggleHeading3",
    hint: "slash.toggleHeading3.hint",
    keywords: ["toggle", "heading", "h3", "collapse", "fold"],
    run: (editor, range) => inBlock(editor, range).setToggleHeading(3).run(),
  },
  {
    id: "quote",
    section: "basic",
    icon: QuoteIcon,
    label: "slash.quote",
    hint: "slash.quote.hint",
    keywords: ["blockquote", "citation"],
    run: (editor, range) => inBlock(editor, range).setBlockquote().run(),
  },
  {
    id: "callout",
    section: "basic",
    icon: CalloutIcon,
    label: "slash.callout",
    hint: "slash.callout.hint",
    keywords: ["note", "info", "tip", "warning"],
    run: (editor, range) => inBlock(editor, range).setCallout().run(),
  },
  {
    id: "divider",
    section: "basic",
    icon: DividerIcon,
    label: "slash.divider",
    hint: "slash.divider.hint",
    keywords: ["hr", "rule", "separator", "line"],
    run: (editor, range) => inBlock(editor, range).setHorizontalRule().run(),
  },

  // ----- Media: pictures and files (the shell's file picker)
  {
    id: "image",
    section: "media",
    icon: ImageIcon,
    label: "slash.image",
    hint: "slash.image.hint",
    keywords: ["picture", "photo", "img", "upload"],
    unavailable: importUnavailable,
    run: (editor, range) => {
      importAttachment(editor, range, "image");
    },
  },
  {
    id: "imageUrl",
    section: "media",
    icon: LinkIcon,
    label: "slash.imageUrl",
    hint: "slash.imageUrl.hint",
    keywords: ["url", "link", "web", "embed", "picture", "photo"],
    // ADR-0028: the shell downloads the picture once. Absent where it cannot (the web build).
    hidden: imageUrlHidden,
    run: (editor, range) => {
      importImageFromUrl(editor, range);
    },
  },
  {
    id: "gallery",
    section: "media",
    icon: GalleryIcon,
    label: "slash.gallery",
    hint: "slash.gallery.hint",
    keywords: ["photos", "pictures", "album"],
    run: (editor, range) => inBlock(editor, range).insertGallery().run(),
  },
  {
    id: "pdf",
    section: "media",
    icon: PdfIcon,
    label: "slash.pdf",
    hint: "slash.pdf.hint",
    keywords: ["document", "read", "acrobat"],
    unavailable: uploadUnavailable,
    run: (editor, range) => {
      insertPdf(editor, range);
    },
  },
  {
    id: "video",
    section: "media",
    icon: VideoIcon,
    label: "slash.video",
    hint: "slash.video.hint",
    keywords: ["movie", "clip", "mp4", "webm", "film"],
    unavailable: uploadUnavailable,
    run: (editor, range) => {
      insertMedia(editor, range, "video");
    },
  },
  {
    id: "audio",
    section: "media",
    icon: AudioIcon,
    label: "slash.audio",
    hint: "slash.audio.hint",
    keywords: ["sound", "music", "podcast", "recording", "mp3", "voice"],
    unavailable: uploadUnavailable,
    run: (editor, range) => {
      insertMedia(editor, range, "audio");
    },
  },
  {
    // Local dictation (ADR-0053): listed while the transport can dictate; the dictation bar takes over. It comes before
    // "Record audio", whose keywords also say microphone and dictate: `/mic`, `/microphone` and `/dictate` find it first.
    id: "microphone",
    section: "media",
    icon: DictateIcon,
    label: "slash.microphone",
    hint: "slash.microphone.hint",
    keywords: ["microphone", "dictate", "dictation", "voice", "speech", "speak", "talk", "stt", "mic"],
    hidden: (editor) => !dictationOn(editor),
    unavailable: dictationUnavailable,
    run: (editor, range) => {
      inBlock(editor, range).run();
      void dictationOf(editor)?.start();
    },
  },
  {
    // Local AI (ADR-0037): listed while AI is on and the browser can record; the recorder bar takes over.
    id: "record",
    section: "media",
    icon: RecordIcon,
    label: "slash.record",
    hint: "slash.record.hint",
    keywords: ["microphone", "voice", "meeting", "capture", "transcript", "transcribe", "dictate"],
    hidden: (editor) => !recordingOn(editor),
    unavailable: recordUnavailable,
    run: (editor, range) => {
      inBlock(editor, range).run();
      void editorAiOf(editor)?.recorder.start();
    },
  },
  {
    id: "file",
    section: "media",
    icon: FileIcon,
    label: "slash.file",
    hint: "slash.file.hint",
    keywords: ["attachment", "upload", "document"],
    unavailable: importUnavailable,
    run: (editor, range) => {
      importAttachment(editor, range, "file");
    },
  },

  // ----- Advanced: code, formulas, structure and derived blocks
  {
    id: "codeBlock",
    section: "advanced",
    icon: CodeIcon,
    label: "slash.codeBlock",
    hint: "slash.codeBlock.hint",
    keywords: ["code", "pre", "snippet"],
    run: (editor, range) => inBlock(editor, range).setCodeBlock().run(),
  },
  {
    id: "diagram",
    section: "advanced",
    icon: DiagramIcon,
    label: "slash.diagram",
    hint: "slash.diagram.hint",
    keywords: ["mermaid", "chart", "flowchart", "sequence"],
    run: insertDiagram,
  },
  {
    id: "whiteboard",
    section: "advanced",
    icon: WhiteboardIcon,
    label: "slash.whiteboard",
    hint: "slash.whiteboard.hint",
    keywords: ["drawing", "draw", "sketch", "canvas", "excalidraw", "paint"],
    run: (editor, range) => inBlock(editor, range).insertWhiteboard().run(),
  },
  {
    id: "math",
    section: "advanced",
    icon: EquationIcon,
    label: "slash.math",
    hint: "slash.math.hint",
    keywords: ["math", "equation", "latex", "formula", "tex"],
    run: (editor, range) => inBlock(editor, range).setMathBlock().run(),
  },
  // Local AI (ADR-0037, the writing assistant): listed while AI is on and a chat model is set.
  {
    id: "ai",
    section: "advanced",
    icon: SparkleIcon,
    label: "slash.ai",
    hint: "slash.ai.hint",
    keywords: ["ai", "write", "assistant", "generate", "llm", "ollama", "ask"],
    hidden: (editor) => !writingOn(editor),
    run: (editor, range) => {
      inBlock(editor, range).run();
      editorAiOf(editor)?.writer.open({ kind: "cursor" });
    },
  },
  {
    id: "aiSummarize",
    section: "advanced",
    icon: SparkleIcon,
    label: "slash.aiSummarize",
    hint: "slash.aiSummarize.hint",
    keywords: ["summary", "tldr", "ai"],
    hidden: (editor) => !writingOn(editor),
    run: (editor, range) => {
      inBlock(editor, range).run();
      editorAiOf(editor)?.writer.open({ kind: "page", action: "summarize" });
    },
  },
  {
    id: "aiActionItems",
    section: "advanced",
    icon: SparkleIcon,
    label: "slash.aiActionItems",
    hint: "slash.aiActionItems.hint",
    keywords: ["action-items", "followup", "ai"],
    hidden: (editor) => !writingOn(editor),
    run: (editor, range) => {
      inBlock(editor, range).run();
      editorAiOf(editor)?.writer.open({ kind: "page", action: "actionItems" });
    },
  },
  {
    id: "table",
    section: "advanced",
    icon: TableIcon,
    label: "slash.table",
    hint: "slash.table.hint", // not worded with "columns": `/columns` is the column layout below
    keywords: ["grid", "rows", "cells"],
    run: insertTable,
  },
  {
    id: "columns2",
    section: "advanced",
    icon: Columns2Icon,
    label: "slash.columns2",
    hint: "slash.columns2.hint",
    keywords: ["columns", "column", "layout", "side by side", "split"],
    unavailable: columnsUnavailable,
    run: (editor, range) => inBlock(editor, range).setColumns(2).run(),
  },
  {
    id: "columns3",
    section: "advanced",
    icon: Columns3Icon,
    label: "slash.columns3",
    hint: "slash.columns3.hint",
    keywords: ["columns", "column", "layout", "side by side", "split"],
    unavailable: columnsUnavailable,
    run: (editor, range) => inBlock(editor, range).setColumns(3).run(),
  },
  // The template button (ADR-0056): a button that holds blocks and inserts a copy of them when it is clicked; its content
  // is open to write. Not inside another button.
  {
    id: "templateButton",
    section: "advanced",
    icon: TemplateButtonIcon,
    label: "slash.templateButton",
    hint: "slash.templateButton.hint",
    keywords: ["button", "repeat", "template", "add", "item", "insert"],
    unavailable: (editor) => (insideTemplateButton(editor.state) ? "slash.templateButton.nested" : null),
    run: (editor, range) => inBlock(editor, range).insertTemplateButton().run(),
  },
  {
    id: "toc",
    section: "advanced",
    icon: TocIcon,
    label: "slash.toc",
    hint: "slash.toc.hint",
    keywords: ["toc", "contents", "headings", "outline"],
    run: (editor, range) => inBlock(editor, range).insertTableOfContents().run(),
  },
  {
    id: "breadcrumb",
    section: "advanced",
    icon: BreadcrumbIcon,
    label: "slash.breadcrumb",
    hint: "slash.breadcrumb.hint",
    keywords: ["breadcrumb", "path", "parents", "ancestors"],
    run: (editor, range) => inBlock(editor, range).insertBreadcrumb().run(),
  },
  {
    id: "pageList",
    section: "advanced",
    icon: ListViewIcon,
    label: "slash.pageList",
    hint: "slash.pageList.hint",
    keywords: ["list", "view", "database", "pages", "children"],
    run: (editor, range) => inBlock(editor, range).insertPageList().run(),
  },
  {
    id: "tableView",
    section: "advanced",
    icon: TableViewIcon,
    label: "slash.tableView",
    hint: "slash.tableView.hint",
    keywords: ["database", "table", "spreadsheet", "view", "properties"],
    run: (editor, range) => inBlock(editor, range).insertPageList("table").run(),
  },
  {
    id: "boardView",
    section: "advanced",
    icon: BoardViewIcon,
    label: "slash.boardView",
    hint: "slash.boardView.hint",
    keywords: ["database", "board", "kanban", "status", "view"],
    run: (editor, range) => inBlock(editor, range).insertPageList("board").run(),
  },
  {
    id: "calendarView",
    section: "advanced",
    icon: CalendarViewIcon,
    label: "slash.calendarView",
    hint: "slash.calendarView.hint",
    keywords: ["database", "calendar", "month", "schedule", "dates", "view"],
    run: (editor, range) => inBlock(editor, range).insertPageList("calendar").run(),
  },
  {
    id: "galleryView",
    section: "advanced",
    icon: GalleryViewIcon,
    label: "slash.galleryView",
    hint: "slash.galleryView.hint",
    keywords: ["database", "gallery", "cards", "covers", "view"],
    run: (editor, range) => inBlock(editor, range).insertPageList("gallery").run(),
  },
  {
    id: "dashboardView",
    section: "advanced",
    icon: DashboardViewIcon,
    label: "slash.dashboardView",
    hint: "slash.dashboardView.hint",
    keywords: ["database", "dashboard", "chart", "number", "statistics", "view"],
    run: (editor, range) => inBlock(editor, range).insertPageList("dashboard").run(),
  },
  // Sorting (ADR-0052): the items of the list the caret is in, A to Z, once.
  {
    id: "sortList",
    section: "advanced",
    icon: SortIcon,
    label: "slash.sortList",
    hint: "slash.sortList.hint",
    keywords: ["sort", "order", "alphabetical", "a-z", "az", "abc"],
    unavailable: (editor) => (innermostListPos(editor.state.doc, editor.state.selection.from) === null ? "slash.sortList.notInList" : null),
    run: sortList,
  },

  // ----- Pages: links to pages and new pages
  {
    id: "pageLink",
    section: "pages",
    icon: LinkIcon,
    label: "slash.pageLink",
    hint: "slash.pageLink.hint",
    // No "page" among the words: `/page` and Enter make a sub-page (below), whose own keyword
    // ranks above this entry's label.
    keywords: ["link", "card", "embed"],
    // The `[[` page picker in card mode: the chosen page becomes a card block (a chip is `[[`).
    run: (editor, range) => {
      openPagePicker(editor, range, true);
    },
  },
  {
    id: "subpage",
    section: "pages",
    icon: SubPageIcon,
    label: "slash.subpage",
    hint: "slash.subpage.hint",
    keywords: ["page", "subpage", "new", "child"],
    unavailable: subPageUnavailable,
    run: (editor, range) => {
      void insertSubPage(editor, range);
    },
  },
  // A sub-page made from a template or a starter (ADR-0056), with its card in the page like `/page`.
  {
    id: "template",
    section: "pages",
    icon: PageFromTemplateIcon,
    label: "template.slash",
    hint: "template.slash.hint",
    // No "page" among the words, for the reason `pageLink` has none: `/page` and Enter make an empty sub-page.
    keywords: ["template", "starter", "preset", "boilerplate"],
    unavailable: templatePageUnavailable,
    run: (editor, range) => {
      void insertSubPageFromTemplate(editor, range);
    },
  },
  // Sorting (ADR-0052): the open page's sub-pages, once, as the sidebar's "Sort subpages" does.
  {
    id: "sortSubpages",
    section: "pages",
    icon: SortIcon,
    label: "slash.sortSubpages",
    hint: "slash.sortSubpages.hint",
    keywords: ["sort", "order", "alphabetical", "a-z", "az", "abc"],
    unavailable: sortSubpagesUnavailable,
    run: (editor, range) => {
      void sortSubpages(editor, range, "title");
    },
  },
  {
    id: "sortSubpagesByDate",
    section: "pages",
    icon: SortIcon,
    label: "slash.sortSubpagesByDate",
    hint: "slash.sortSubpagesByDate.hint",
    keywords: ["sort", "order", "date", "created", "creation", "chronological", "oldest"],
    unavailable: sortSubpagesUnavailable,
    run: (editor, range) => {
      void sortSubpages(editor, range, "created");
    },
  },

  // ----- Inline: what goes inside text (a date, a mention, an emoji, an equation)
  {
    id: "date",
    section: "inline",
    icon: CalendarIcon,
    label: "slash.date",
    hint: "slash.date.hint",
    keywords: ["date", "today", "tomorrow", "calendar", "when"],
    unavailable: (editor) => (takesInlineAtom(editor, "dateMention") ? null : "slash.inline.notHere"),
    // A mention of today, with its picker open: a day picked there replaces it.
    run: (editor, range) => {
      editor.chain().focus().deleteRange(range).insertDateMention({ date: todayIso(), openPicker: true }).run();
    },
  },
  {
    id: "mention",
    section: "inline",
    icon: AtIcon,
    label: "slash.mention",
    hint: "slash.mention.hint",
    keywords: ["mention", "at", "page", "person"],
    unavailable: (editor) =>
      takesInlineAtom(editor, "pageRef") || takesInlineAtom(editor, "dateMention") ? null : "slash.inline.notHere",
    // The `@` menu: it opens on the `@` this types, as `/link to page` opens the `[[` picker.
    run: (editor, range) => {
      openMentionMenu(editor, range);
    },
  },
  {
    id: "emoji",
    section: "inline",
    icon: EmojiIcon,
    label: "slash.emoji",
    hint: "slash.emoji.hint",
    keywords: ["emoji", "emoticon", "smiley", "icon"],
    // The `:` emoji picker, opened on its default list: the chosen emoji is typed as text.
    run: (editor, range) => {
      openEmojiPicker(editor, range);
    },
  },
  {
    id: "mathInline",
    section: "inline",
    icon: InlineEquationIcon,
    label: "slash.mathInline",
    hint: "slash.mathInline.hint",
    keywords: ["inline", "equation", "math", "latex", "formula"],
    unavailable: (editor) => (takesInlineAtom(editor, "mathInline") ? null : "slash.inline.notHere"),
    // An empty equation with its popover open: the formula typed there fills it in. (`/equation` is the
    // block, which ranks first for the words the two share.) No `focus()` in the chain: it hands the
    // focus back to the page a frame later, which would take it from the popover's field.
    run: (editor, range) => {
      editor.chain().deleteRange(range).insertMathInline({ openEditor: true }).run();
    },
  },
];

/** Position of each section in `SLASH_SECTIONS`. */
const SECTION_ORDER: ReadonlyMap<SlashSection, number> = new Map(
  SLASH_SECTIONS.map((section, index) => [section, index]),
);

/**
 * Every entry, grouped by section in the order of `SLASH_SECTIONS` (the order of `ENTRIES`
 * within a section, whatever the order of the sections in the source).
 */
export const SLASH_COMMANDS: readonly SlashCommand[] = [...ENTRIES].sort(
  (a, b) => (SECTION_ORDER.get(a.section) ?? 0) - (SECTION_ORDER.get(b.section) ?? 0),
);

/** Lowercase, without spaces: how labels, words and what is typed are compared. */
function normalise(text: string): string {
  return text.toLowerCase().replace(/\s+/g, "");
}

/**
 * How well a command matches what was typed (already normalised): the `tier` is 1 when its
 * label starts with it, 2 when a keyword does, 3 when its label contains it, 4 when a keyword
 * or its hint does; the `size` is the length of the text that matched (the shortest one) in the
 * first three tiers, since the match that covers more of the text is the closer one, and 0
 * in the last, where the order of the list decides.
 */
interface Match {
  tier: 1 | 2 | 3 | 4;
  size: number;
}

function matchOf(command: SlashCommand, wanted: string): Match | null {
  const label = normalise(t(command.label));
  if (label.startsWith(wanted)) {
    return { tier: 1, size: label.length };
  }
  const words = command.keywords.map(normalise);
  const shortest = (texts: readonly string[]): number => Math.min(...texts.map((text) => text.length));
  const prefixed = words.filter((word) => word.startsWith(wanted));
  if (prefixed.length > 0) {
    return { tier: 2, size: shortest(prefixed) };
  }
  if (label.includes(wanted)) {
    return { tier: 3, size: label.length };
  }
  const found = words.some((word) => word.includes(wanted)) || normalise(t(command.hint)).includes(wanted);
  return found ? { tier: 4, size: 0 } : null;
}

/**
 * The commands that match what was typed after `/`, best first: label prefix, keyword
 * prefix, label substring, then keyword or hint substring; in the first three, the shorter
 * text first (`/to` finds Toggle before To-do list), then the order of the list. Matching
 * ignores case and spaces. Nothing typed gives the whole list (grouped by section).
 */
export function filterSlashCommands(query: string): SlashCommand[] {
  const wanted = normalise(query);
  if (wanted === "") {
    return [...SLASH_COMMANDS];
  }
  const matches: { command: SlashCommand; match: Match }[] = [];
  for (const command of SLASH_COMMANDS) {
    const match = matchOf(command, wanted);
    if (match !== null) {
      matches.push({ command, match });
    }
  }
  return matches
    .sort((a, b) => a.match.tier - b.match.tier || a.match.size - b.match.size) // stable: the rest keeps the list order
    .map(({ command }) => command);
}

export interface SlashCommandsOptions {
  /** Where the menu shows; `null` disables the menu (schema-only editors). */
  store: SuggestionMenuStore<SlashCommand> | null;
}

export const slashMenuKey = new PluginKey("slashMenu");

/** `/` at the start of a block or after a space opens the block menu (not in code blocks). */
export const SlashCommands = Extension.create<SlashCommandsOptions>({
  name: "slashCommands",

  addOptions() {
    return { store: null };
  },

  addProseMirrorPlugins() {
    const { store } = this.options;
    if (store === null) {
      return [];
    }
    return [
      Suggestion<SlashCommand, SlashCommand>({
        editor: this.editor,
        pluginKey: slashMenuKey,
        char: "/",
        items: ({ query, editor }) => filterSlashCommands(query).filter((command) => command.hidden?.(editor) !== true),
        command: ({ editor, range, props }) => {
          if (props.unavailable?.(editor) == null) {
            props.run(editor, range);
          }
        },
        allow: ({ state, range }) => state.doc.resolve(range.from).parent.type.spec.code !== true,
        render: () => store.renderer(),
      }),
    ];
  },
});
