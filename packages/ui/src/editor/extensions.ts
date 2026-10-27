// The editor's extensions: block schema v1 (`schema.ts`) plus the behaviour around it,
// the Loro binding first among them.

import { combineTransactionSteps, Extension, getChangedRanges } from "@tiptap/core";
import type { AnyExtension } from "@tiptap/core";
import type { Node as PmNode } from "@tiptap/pm/model";
import { Plugin, PluginKey } from "@tiptap/pm/state";
import type { Command, EditorState } from "@tiptap/pm/state";
import { Decoration, DecorationSet } from "@tiptap/pm/view";
import { ReactNodeViewRenderer } from "@tiptap/react";
import { WHITEBOARD_ORIGIN } from "@atomik-kb/core-client";
import type { PersonInfo } from "@atomik-kb/core-client";
import { UndoManager } from "loro-crdt";
import type { LoroDoc } from "loro-crdt";

import { t } from "../i18n";
import type { PageChoice } from "../store-content";
import { Attachments } from "./attachments";
import type { AttachmentService } from "./attachments";
import { FileView, ImageView } from "./AttachmentViews";
import { pdfStopsEvent } from "./PdfViewer";
import { galleryStopsEvent, GalleryView } from "./GalleryView";
import { BlockLinkPaste } from "./block-link";
import { BlockSelection } from "./block-selection";
import { BreadcrumbView } from "./BreadcrumbView";
import { CommentHighlights } from "./comment-highlights";
import { DateMentionView } from "./DateMentionView";
import type { EmojiChoice } from "./emoji-data";
import { EmojiPicker } from "./emoji-picker";
import { FindReplace } from "./find-replace";
import { LoroSyncPlugin, LoroUndoPlugin, redo, undo } from "./loro-binding";
import type { LoroDocType } from "./loro-binding";
import { MarkdownPaste } from "./markdown-paste";
import { MathInlineView } from "./MathInlineView";
import { MentionMenu } from "./mention-menu";
import { PageDrop } from "./page-drop";
import { BlockPlaceholder } from "./placeholder";
import type { MentionChoice } from "./mention-menu";
import { CalloutView, PageLinkView, PageRefView } from "./node-views";
import { PageListView } from "./PageListView";
import { PagePicker } from "./page-picker";
import type { PagePresence } from "./page-presence";
import { PresenceCursors } from "./presence-cursors";
import { PersonMentionView } from "./PersonMentionView";
import { createSchemaExtensions, isBindingTransaction } from "./schema";
import { SlashCommands } from "./slash-commands";
import type { SlashCommand } from "./slash-commands";
import { SubPages } from "./sub-page";
import type { SubPageService } from "./sub-page";
import type { SuggestionMenuStore } from "./suggestion-menu";
import { TableOfContentsView } from "./TableOfContentsView";
import { TemplateButtonView } from "./TemplateButtonView";
import { createWhiteboardStore } from "./whiteboard-store";
import { WhiteboardCopy } from "./whiteboard-copy";
import { whiteboardStopsEvent, WhiteboardView } from "./WhiteboardView";

/** Consecutive local changes within this time undo together. */
const UNDO_MERGE_MS = 500;
const UNDO_STEPS = 200;

export interface LoroBindingOptions {
  /** The page's replica; `null` adds nothing (schema-only editors). */
  doc: LoroDoc | null;
  /** Never write to the document (content outside the schema). */
  readOnly: boolean;
}

export interface LoroBindingStorage {
  undoManager: UndoManager | null;
  /**
   * Runs `edit` (a dispatch that changes the page) so that its change is an undo step of its own, whatever the merge
   * interval says, and so that the next change after it (a keystroke a moment later) starts another step instead of
   * joining it: Ctrl+Z then takes away that change and nothing else (dictation, ADR-0053: one step per phrase). The
   * undo manager merges a change into the step before it when the step began less than `UNDO_MERGE_MS` ago, and
   * has no call that closes the step; the interval is set to 0 for `edit` and stays 0 until the next change of the page
   * is in (or `UNDO_MERGE_MS` have passed, after which the interval would not have merged it anyway). Without a
   * document (a schema-only editor) it just runs `edit`.
   */
  asOwnUndoStep: (edit: () => void) => void;
  /** Called after every change a person or a command made to the page: ends what `asOwnUndoStep` held back. */
  afterLocalChange: () => void;
}

/**
 * The vendored Loro binding as Tiptap extension: the sync plugin, Loro's undo manager
 * (Tiptap's own undo is off, ADR-0014) and its keys: Mod-z undoes, Mod-Shift-z and Mod-y
 * redo. The keys are always taken, so the browser's own undo never touches the document.
 */
export const LoroBinding = Extension.create<LoroBindingOptions, LoroBindingStorage>({
  name: "loroBinding",

  addOptions() {
    return { doc: null, readOnly: false };
  },

  addStorage() {
    return {
      undoManager: null,
      asOwnUndoStep: (edit) => {
        edit();
      },
      afterLocalChange: () => undefined,
    };
  },

  addProseMirrorPlugins() {
    const { doc, readOnly } = this.options;
    if (doc === null) {
      return [];
    }
    // A drawing has its own history (Excalidraw's, in its editor): the commits of its writes carry the origin
    // `whiteboard` and page undo skips them (ADR-0033).
    const undoManager = new UndoManager(doc, {
      mergeInterval: UNDO_MERGE_MS,
      maxUndoSteps: UNDO_STEPS,
      excludeOriginPrefixes: [WHITEBOARD_ORIGIN],
    });
    this.storage.undoManager = undoManager;
    // `asOwnUndoStep`: the interval is 0 from the start of the edit until the next local change (or the end of the window).
    let held = false;
    let releaseTimer: ReturnType<typeof setTimeout> | undefined;
    const release = (): void => {
      if (held) {
        held = false;
        clearTimeout(releaseTimer);
        undoManager.setMergeInterval(UNDO_MERGE_MS);
      }
    };
    this.storage.afterLocalChange = release;
    this.storage.asOwnUndoStep = (edit) => {
      release();
      undoManager.setMergeInterval(0);
      try {
        edit();
      } catch (error) {
        undoManager.setMergeInterval(UNDO_MERGE_MS);
        throw error;
      }
      held = true;
      releaseTimer = setTimeout(release, UNDO_MERGE_MS);
    };
    const runEdit = (command: Command) => (state: Parameters<Command>[0], dispatch: Parameters<Command>[1]) => {
      command(state, dispatch);
      return true;
    };
    return [
      LoroSyncPlugin({ doc: doc as LoroDocType, readOnly }),
      LoroUndoPlugin({ doc, undoManager }),
      new Plugin({
        key: new PluginKey("loroHistoryInput"),
        props: {
          handleDOMEvents: {
            // The browser's own undo (menu, gesture) arrives as `beforeinput`.
            beforeinput: (view, event) => {
              if (event.inputType === "historyUndo" || event.inputType === "historyRedo") {
                event.preventDefault();
                runEdit(event.inputType === "historyUndo" ? undo : redo)(view.state, view.dispatch);
                return true;
              }
              return false;
            },
          },
        },
      }),
    ];
  },

  addKeyboardShortcuts() {
    const run = (command: Command) => () => {
      command(this.editor.state, this.editor.view.dispatch);
      return true;
    };
    return { "Mod-z": run(undo), "Shift-Mod-z": run(redo), "Mod-y": run(redo) };
  },

  onTransaction({ transaction }) {
    // A change of the page that is not the binding showing the document: the first one after `asOwnUndoStep` ends its hold.
    if (transaction.docChanged && !isBindingTransaction(transaction)) {
      this.storage.afterLocalChange();
    }
  },

  onDestroy() {
    const { undoManager } = this.storage;
    // Ends a hold of `asOwnUndoStep` (and its timer) while the manager is still alive.
    this.storage.afterLocalChange();
    this.storage.afterLocalChange = () => undefined;
    this.storage.undoManager = null;
    // Tiptap calls this before it destroys the view, whose undo plugin still unhooks
    // itself from the manager then: free it once that is done.
    queueMicrotask(() => {
      undoManager?.free();
    });
  },
});

/**
 * Marks stay on text. Shape v1 stores marks on text only, so a mark ProseMirror puts on a
 * page reference or a line break (bold over a sentence with a chip) would show until the
 * next reload; this removes it at once.
 */
export const TextOnlyMarks = Extension.create({
  name: "textOnlyMarks",

  addProseMirrorPlugins() {
    return [
      new Plugin({
        key: new PluginKey("textOnlyMarks"),
        appendTransaction: (transactions, oldState, newState) => {
          if (!transactions.some((tr) => tr.docChanged) || transactions.some(isBindingTransaction)) {
            return null;
          }
          const tr = newState.tr;
          for (const { newRange } of getChangedRanges(combineTransactionSteps(oldState.doc, [...transactions]))) {
            newState.doc.nodesBetween(newRange.from, newRange.to, (node: PmNode, pos: number) => {
              if (node.isInline && !node.isText && node.marks.length > 0) {
                tr.setNodeMarkup(pos, undefined, node.attrs, []);
              }
            });
          }
          return tr.docChanged ? tr : null;
        },
      }),
    ];
  },
});

export const blockHighlightKey = new PluginKey<DecorationSet>("blockHighlight");

/**
 * Highlights one block for a moment (the block a search result or a backlink led to). A
 * decoration, because ProseMirror redraws a node whose DOM is changed from outside.
 * Dispatch `tr.setMeta(blockHighlightKey, pos)` to show it and `null` to clear it.
 */
export const BlockHighlight = Extension.create({
  name: "blockHighlight",

  addProseMirrorPlugins() {
    return [
      new Plugin<DecorationSet>({
        key: blockHighlightKey,
        state: {
          init: () => DecorationSet.empty,
          apply: (tr, decorations, _old, state: EditorState) => {
            const meta: unknown = tr.getMeta(blockHighlightKey);
            if (meta === null) {
              return DecorationSet.empty;
            }
            if (typeof meta === "number") {
              const node = state.doc.nodeAt(meta);
              return node === null
                ? DecorationSet.empty
                : DecorationSet.create(state.doc, [
                    Decoration.node(meta, meta + node.nodeSize, { class: "is-target" }),
                  ]);
            }
            return decorations.map(tr.mapping, tr.doc);
          },
        },
        props: {
          decorations: (state) => blockHighlightKey.getState(state),
        },
      }),
    ];
  },
});

export interface LinkClickOptions {
  open: (href: string) => void;
}

/**
 * A click on a link opens it: http, https and mailto through the shell (or a new tab in
 * the browser), nothing else. The webview itself never follows a link from a page.
 */
export const LinkClick = Extension.create<LinkClickOptions>({
  name: "linkClick",

  addOptions() {
    return { open: () => undefined };
  },

  addProseMirrorPlugins() {
    const { open } = this.options;
    return [
      new Plugin({
        key: new PluginKey("linkClick"),
        props: {
          handleDOMEvents: {
            click: (view, event) => {
              const target = event.target instanceof Element ? event.target.closest("a[href]") : null;
              if (target === null || !view.dom.contains(target) || event.button !== 0) {
                return false;
              }
              event.preventDefault();
              open(target.getAttribute("href") ?? "");
              return true;
            },
            auxclick: (view, event) => {
              const target = event.target instanceof Element ? event.target.closest("a[href]") : null;
              if (target !== null && view.dom.contains(target)) {
                event.preventDefault(); // no new window from a middle click
                return true;
              }
              return false;
            },
          },
        },
      }),
    ];
  },
});

export interface EditorExtensionOptions {
  doc: LoroDoc;
  /** The page the editor shows (the breadcrumb block draws the chain that ends at it). */
  pageId: string;
  readOnly: boolean;
  slashMenu: SuggestionMenuStore<SlashCommand>;
  pagePicker: SuggestionMenuStore<PageChoice>;
  /** Where the `:` emoji picker shows (`/emoji` opens it too); absent or `null`: no emoji picker. */
  emojiPicker?: SuggestionMenuStore<EmojiChoice> | null;
  /** Where the `@` menu (days and pages) shows (`/mention` opens it too); absent or `null`: no `@` menu. */
  mentionMenu?: SuggestionMenuStore<MentionChoice> | null;
  findPages: (query: string) => Promise<PageChoice[]>;
  openLink: (href: string) => void;
  /** The current title of a page, for references copied as HTML or text. */
  pageTitle: (pageId: string) => string | null;
  /** Uploads into the page's space (paste, drop, the `/` commands); `null` adds none. */
  attachments: AttachmentService | null;
  /** The file name of an attachment when known, for file blocks copied as HTML or text. */
  attachmentName: (attachmentId: string) => string | null;
  /** Creates and opens sub-pages of the open page (`/page`); absent or `null`: the command is unavailable. */
  subPages?: SubPageService | null;
  /** The space of the page: the members of it are who a person mention names (ADR-0049); absent: the chip shows its label. */
  spaceId?: string | null;
  /** The people the `@` menu offers (the space's members); absent: none. */
  findPeople?: () => Promise<readonly PersonInfo[]>;
  /** Whether the page is a template or inside one (ADR-0056; asked when the `@` menu opens): the menu then offers "Me (whoever creates)"; absent: no. */
  inTemplate?: () => boolean;
  /** A click on the highlight of a comment thread (ADR-0048); absent: nothing happens. */
  onOpenThread?: ((threadId: string) => void) | null;
  /** The page's presence in a shared space (Milestone 13, slice 3): the other members' cursors are drawn and this editor's selection is sent; absent or `null`: none. */
  presence?: PagePresence | null;
}

function placeholderFor(node: PmNode): string {
  switch (node.type.name) {
    case "paragraph":
      return t("editor.placeholder");
    case "heading":
      return t("editor.placeholder.heading", { level: Number(node.attrs.level) });
    case "detailsSummary":
      return t("editor.placeholder.toggle");
    default:
      return "";
  }
}

/** Everything the page editor runs: the schema, the binding and the block UX. */
export function createEditorExtensions(options: EditorExtensionOptions): AnyExtension[] {
  const whiteboardStore = createWhiteboardStore(options.doc);
  return [
    ...createSchemaExtensions({
      pageRef: {
        nodeView: ReactNodeViewRenderer(PageRefView, {
          as: "span",
          className: "page-ref-node",
          attrs: ({ HTMLAttributes }) => ({ ...stringAttributes(HTMLAttributes), "data-page-ref": "" }),
        }),
        label: options.pageTitle,
      },
      dateMention: {
        nodeView: ReactNodeViewRenderer(DateMentionView, {
          as: "span",
          className: "date-mention-node",
          attrs: ({ HTMLAttributes }) => ({ ...stringAttributes(HTMLAttributes), "data-type": "dateMention" }),
        }),
        spaceId: options.spaceId ?? null,
      },
      personMention: {
        nodeView: ReactNodeViewRenderer(PersonMentionView, {
          as: "span",
          className: "person-mention-node",
          attrs: ({ HTMLAttributes }) => ({ ...stringAttributes(HTMLAttributes), "data-type": "personMention" }),
        }),
        spaceId: options.spaceId ?? null,
      },
      mathInline: {
        nodeView: ReactNodeViewRenderer(MathInlineView, {
          as: "span",
          className: "math-inline-node",
          attrs: ({ HTMLAttributes }) => ({ ...stringAttributes(HTMLAttributes), "data-type": "mathInline" }),
        }),
      },
      pageLink: {
        nodeView: ReactNodeViewRenderer(PageLinkView, {
          className: "page-link-node",
          attrs: ({ HTMLAttributes }) => ({ ...stringAttributes(HTMLAttributes), "data-type": "pageLink" }),
        }),
        label: options.pageTitle,
      },
      callout: {
        nodeView: ReactNodeViewRenderer(CalloutView, {
          className: "callout",
          attrs: ({ HTMLAttributes }) => {
            const attributes = stringAttributes(HTMLAttributes);
            // Tiptap sets these attributes on the view's element and never removes one, so
            // going back to the default colour has to overwrite `data-color`, not drop it.
            return { ...attributes, "data-type": "callout", "data-color": attributes["data-color"] ?? "" };
          },
        }),
      },
      // The template button (ADR-0056): the button, the fold-out content and its settings are the view's own; "me" is found
      // among the people of the page's space.
      templateButton: {
        nodeView: ReactNodeViewRenderer(TemplateButtonView, {
          className: "template-button",
          attrs: ({ HTMLAttributes }) => ({ ...stringAttributes(HTMLAttributes), "data-type": "templateButton" }),
        }),
        spaceId: options.spaceId ?? null,
        findPeople: options.findPeople ?? (() => Promise.resolve([])),
      },
      image: {
        nodeView: ReactNodeViewRenderer(ImageView, {
          className: "attachment-node",
          attrs: ({ HTMLAttributes }) => {
            const attributes = stringAttributes(HTMLAttributes);
            // Tiptap never removes an attribute from the view's element, so the ones an edit
            // can unset (the layout of ADR-0028) are overwritten, not dropped, or they would go stale.
            const layout = Object.fromEntries(
              ["data-width", "data-width-percent", "data-align", "data-caption"].map((name) => [name, attributes[name] ?? ""]),
            );
            return { ...attributes, ...layout, "data-type": "image" };
          },
        }),
      },
      // The photo gallery (ADR-0030). The drags of its own pictures are its own; its data-items attribute
      // is left off the view's element (a long JSON text the view has no use for).
      gallery: {
        nodeView: ReactNodeViewRenderer(GalleryView, {
          className: "attachment-node",
          attrs: ({ HTMLAttributes }) => {
            const attributes = stringAttributes(HTMLAttributes);
            delete attributes["data-items"];
            return { ...attributes, "data-type": "gallery" };
          },
          stopEvent: ({ event }) => galleryStopsEvent(event),
        }),
      },
      // The whiteboard (ADR-0033): the picture and the buttons are the view's own; the drawing's elements are read
      // from and written to the page document through the store.
      whiteboard: {
        nodeView: ReactNodeViewRenderer(WhiteboardView, {
          className: "attachment-node",
          attrs: ({ HTMLAttributes }) => ({ ...stringAttributes(HTMLAttributes), "data-type": "whiteboard" }),
          stopEvent: ({ event }) => whiteboardStopsEvent(event),
        }),
        store: whiteboardStore,
      },
      file: {
        nodeView: ReactNodeViewRenderer(FileView, {
          className: "attachment-node",
          attrs: ({ HTMLAttributes }) => ({ ...stringAttributes(HTMLAttributes), "data-type": "file" }),
          // A PDF preview (ADR-0031) handles its own clicks, keys, wheel and text selection.
          stopEvent: ({ event }) => pdfStopsEvent(event),
        }),
        name: options.attachmentName,
      },
      tableOfContents: {
        nodeView: ReactNodeViewRenderer(TableOfContentsView, {
          className: "toc-node",
          attrs: ({ HTMLAttributes }) => ({ ...stringAttributes(HTMLAttributes), "data-type": "tableOfContents" }),
        }),
      },
      breadcrumb: {
        nodeView: ReactNodeViewRenderer(BreadcrumbView, {
          className: "breadcrumb-node",
          attrs: ({ HTMLAttributes }) => ({ ...stringAttributes(HTMLAttributes), "data-type": "breadcrumb" }),
        }),
        pageId: options.pageId,
      },
      pageList: {
        nodeView: ReactNodeViewRenderer(PageListView, {
          className: "page-list-node",
          attrs: ({ HTMLAttributes }) => ({ ...stringAttributes(HTMLAttributes), "data-type": "pageList" }),
        }),
        pageId: options.pageId,
      },
      assignIds: !options.readOnly,
    }),
    Attachments.configure({ service: options.readOnly ? null : options.attachments }),
    // A duplicated or pasted whiteboard takes a copy of its drawing along (ADR-0033).
    WhiteboardCopy.configure({ store: options.readOnly ? null : whiteboardStore }),
    // Paste Markdown as blocks: right after the file paste above (that one takes files and leaves
    // text alone, this one takes text and leaves HTML and files alone), so they never compete.
    // `BlockLinkPaste` at the end takes a bare link of the app, which is no Markdown to this one.
    MarkdownPaste,
    BlockPlaceholder.configure({ placeholder: ({ node }) => placeholderFor(node) }),
    LoroBinding.configure({ doc: options.doc, readOnly: options.readOnly }),
    // The cursors of the other members (ADR-0050): after the binding, whose state the cursor plugin reads.
    PresenceCursors.configure({ presence: options.presence ?? null }),
    TextOnlyMarks,
    BlockHighlight,
    // The highlights of the anchored comment threads (ADR-0048): decorations, so that a Commenter sees them too.
    CommentHighlights.configure({ onOpenThread: options.onOpenThread ?? null }),
    // Selecting, extending and clearing a range of top-level blocks (Escape, Shift+Up/Down, Shift+click).
    BlockSelection,
    // Find in page and replace: the matches of the find bar drawn over the text (`FindBar.tsx` drives it).
    FindReplace,
    LinkClick.configure({ open: options.openLink }),
    SlashCommands.configure({ store: options.slashMenu }),
    PagePicker.configure({ store: options.pagePicker, findPages: options.findPages }),
    MentionMenu.configure({
      store: options.mentionMenu ?? null,
      findPages: options.findPages,
      findPeople: options.findPeople ?? (() => Promise.resolve([])),
      inTemplate: options.inTemplate ?? (() => false),
    }),
    PageDrop.configure({ spaceId: options.spaceId ?? null, pageId: options.pageId }),
    EmojiPicker.configure({ store: options.emojiPicker ?? null }),
    SubPages.configure({ service: options.readOnly ? null : (options.subPages ?? null) }),
    BlockLinkPaste,
  ];
}

/** Rendered attributes as strings (node view wrappers set them on a DOM element). */
function stringAttributes(attributes: Record<string, unknown>): Record<string, string> {
  const result: Record<string, string> = {};
  for (const [key, value] of Object.entries(attributes)) {
    if (value !== null && value !== undefined && value !== false) {
      result[key] = typeof value === "string" ? value : JSON.stringify(value);
    }
  }
  return result;
}
