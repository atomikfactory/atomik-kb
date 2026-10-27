// Getting files into a page (milestone-05.md, "UI"): paste of image data, drop of files, and
// the "Image" and "File" commands of the `/` menu (the shell's file picker). A file is
// uploaded first and its block inserted afterwards, so the page document never holds a block
// without an attachment: while the upload runs, a placeholder card with its progress stands
// where the block will go (a widget decoration, mapped through every edit), and a refused
// upload (too large, a name or type the core does not accept) leaves an error card there
// until it is dismissed. Several files upload one after the other and land in order.

import { Extension } from "@tiptap/core";
import type { Editor, Range } from "@tiptap/core";
import type { Node as PmNode, NodeType } from "@tiptap/pm/model";
import { NodeSelection, Plugin, PluginKey } from "@tiptap/pm/state";
import type { EditorState } from "@tiptap/pm/state";
import { Decoration, DecorationSet } from "@tiptap/pm/view";
import type { EditorView } from "@tiptap/pm/view";
import { MAX_ATTACHMENT_BYTES, MAX_PREVIEW_BYTES, toCoreError } from "@atomik-kb/core-client";
import type { AttachmentInfo, UploadProgress } from "@atomik-kb/core-client";

import { detailOf } from "../errors";
import { formatBytes } from "../format";
import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { FILE_NODE, IMAGE_NODE, isPdfFile, mediaKindOf } from "./attachment-nodes";
import { GALLERY_NODE } from "./gallery-nodes";
import { addImagesToGallery, canAddToGallery } from "./gallery-uploads";
import { BLOCK_ID_ATTRIBUTE } from "./schema";

/** What the editor needs from the app to add files: uploads into the page's space. */
export interface AttachmentService {
  /** Pasted and dropped files can be uploaded. */
  canUpload: boolean;
  /** The shell's file picker is available (the `/` commands). */
  canImport: boolean;
  /** Uploads a pasted or dropped file; rejects with a `CoreError`. */
  upload: (file: File, onProgress: UploadProgress) => Promise<AttachmentInfo>;
  /** Lets the user pick a file and uploads it; `null` when the dialog was cancelled. */
  importFile: () => Promise<AttachmentInfo | null>;
  /**
   * The shell can download a picture from a link (the `/` command "Image from URL", ADR-0028);
   * without it the command is not in the menu at all.
   */
  canImportUrl: boolean;
  /**
   * Has the shell download the picture an https link points to, once, and store it as an
   * attachment; rejects with a `CoreError` (`InvalidInput` for the field `url` with
   * `details.reason`, or for `size`). The link goes to the shell only.
   */
  importUrl: (url: string) => Promise<AttachmentInfo>;
  /** Asks the person for the link in a dialog: the text typed, or `null` when they cancelled. */
  askUrl: () => Promise<string | null>;
}

export type AttachmentKind = "image" | "file";

export interface AttachmentsOptions {
  /** `null` adds nothing (schema-only and read-only editors). */
  service: AttachmentService | null;
}

export interface AttachmentsStorage {
  service: AttachmentService | null;
}

declare module "@tiptap/core" {
  interface Storage {
    attachments: AttachmentsStorage;
  }
}

export const attachmentUploadsKey = new PluginKey<DecorationSet>("attachmentUploads");

type UploadMeta = { add: { id: string; pos: number; element: HTMLElement; order: number } } | { remove: string };

interface PlaceholderSpec {
  uploadId: string;
}

/** Increases with every placeholder, so that placeholders at one position keep their order. */
let placeholderOrder = 0;

/** The service of an editor, if it has the extension. */
export function serviceOf(editor: Editor): AttachmentService | null {
  // Absent in editors built without the extension (schema-only uses).
  const storage = (editor.storage as unknown as { attachments?: AttachmentsStorage }).attachments;
  return storage?.service ?? null;
}

/**
 * Why the `/` commands that pick a file are unavailable, or `null` when they work: the shell
 * has no file picker (drop and paste may still work), or attachments are not supported at all.
 */
export function importUnavailable(editor: Editor): MessageKey | null {
  const service = serviceOf(editor);
  if (service?.canImport === true) {
    return null;
  }
  return service?.canUpload === true ? "slash.attachment.dropInstead" : "slash.attachment.unsupported";
}

/**
 * Why the `/` command "PDF" is unavailable, or `null` when it works. It picks the file with the
 * webview's own chooser and uploads it like a dropped file, so it needs uploads, not the shell's
 * picker (ADR-0031).
 */
export function uploadUnavailable(editor: Editor): MessageKey | null {
  return serviceOf(editor)?.canUpload === true ? null : "slash.attachment.unsupported";
}

/**
 * Whether the `/` command "Image from URL" is left out of the menu: the editor has no service,
 * or its shell cannot download a link (the web build will not: CORS). Unlike "Image" and "File",
 * which stay and say why they cannot run, there is nothing the person could do about this one.
 */
export function imageUrlHidden(editor: Editor): boolean {
  return serviceOf(editor)?.canImportUrl !== true;
}

// ----- where a block goes ---------------------------------------------------------------

function canInsertAt(doc: PmNode, pos: number, type: NodeType): boolean {
  const $pos = doc.resolve(pos);
  const index = $pos.index();
  return $pos.parent.canReplaceWith(index, index, type);
}

/** Where a new attachment block goes for a position, and the empty paragraph it may replace. */
export interface Placement {
  pos: number;
  /** Block id of an empty paragraph the first block takes the place of. */
  replace: string | null;
}

/**
 * The block position nearest to `pos` where a block of `type` fits: `pos` itself between
 * blocks; before the textblock `pos` is in when at its start (or when it is empty, which the
 * new block then replaces), after it otherwise; further out when the parent does not take the
 * block (a toggle's summary); the end of the page as a last resort.
 */
export function placementFor(doc: PmNode, pos: number, type: NodeType): Placement {
  const $pos = doc.resolve(Math.max(0, Math.min(pos, doc.content.size)));
  if (canInsertAt(doc, $pos.pos, type)) {
    return { pos: $pos.pos, replace: null };
  }
  const parent = $pos.parent;
  const empty = parent.isTextblock && parent.content.size === 0;
  const before = empty || (parent.isTextblock && $pos.parentOffset === 0);
  for (let depth = $pos.depth; depth > 0; depth -= 1) {
    const at = before ? $pos.before(depth) : $pos.after(depth);
    if (canInsertAt(doc, at, type)) {
      const id: unknown = parent.attrs[BLOCK_ID_ATTRIBUTE];
      const replaceable = empty && depth === $pos.depth && parent.type.name === "paragraph" && typeof id === "string";
      return { pos: at, replace: replaceable ? id : null };
    }
  }
  return { pos: doc.content.size, replace: null };
}

// ----- placeholder cards ------------------------------------------------------------------

/** The card that stands where a block will be, while its file uploads. */
class UploadCard {
  readonly id = crypto.randomUUID();
  readonly element: HTMLElement;
  readonly #status: HTMLElement;
  readonly #bar: HTMLProgressElement;

  /** `total` is the file's size, or `null` while unknown (an indeterminate bar). */
  constructor(name: string, label: string, total: number | null) {
    this.element = document.createElement("div");
    this.element.className = "attachment-upload";
    this.element.contentEditable = "false";
    this.element.dataset.upload = this.id;

    const icon = document.createElement("span");
    icon.className = "attachment-icon";
    icon.setAttribute("aria-hidden", "true");
    icon.textContent = "⏳";

    const text = document.createElement("div");
    text.className = "attachment-upload-text";
    const title = document.createElement("span");
    title.className = "attachment-name";
    title.textContent = name;
    this.#status = document.createElement("span");
    this.#status.className = "attachment-upload-status";
    text.append(title, this.#status);

    this.#bar = document.createElement("progress");
    this.#bar.className = "attachment-upload-bar";
    this.#bar.setAttribute("aria-label", label);
    if (total !== null) {
      this.#bar.max = Math.max(1, total);
      this.#bar.value = 0;
      this.#status.textContent = t("attachment.upload.percent", { percent: 0 });
    }
    text.append(this.#bar);
    this.element.append(icon, text);
  }

  /** Keeps the card out of sight (it already holds its place in the page) until {@link UploadCard.reveal}. */
  conceal(): void {
    this.element.style.display = "none";
  }

  reveal(): void {
    this.element.style.display = "";
  }

  /** A line of status under the title, for a card whose progress is not a percentage. */
  note(text: string): void {
    this.#status.textContent = text;
  }

  progress(sent: number, total: number): void {
    const max = Math.max(1, total);
    this.#bar.max = max;
    this.#bar.value = Math.min(sent, max);
    this.#status.textContent = t("attachment.upload.percent", { percent: Math.floor((Math.min(sent, max) / max) * 100) });
  }

  fail(message: string, onDismiss: () => void): void {
    this.element.classList.add("is-error");
    this.#bar.remove();
    const icon = this.element.querySelector(".attachment-icon");
    if (icon !== null) {
      icon.textContent = "⚠️";
    }
    this.#status.textContent = message;
    this.#status.setAttribute("role", "alert");
    const dismiss = document.createElement("button");
    dismiss.type = "button";
    dismiss.className = "button attachment-upload-dismiss";
    dismiss.textContent = t("attachment.dismiss");
    dismiss.addEventListener("click", (event) => {
      event.preventDefault();
      onDismiss();
    });
    this.element.append(dismiss);
  }
}

function addPlaceholder(view: EditorView, pos: number, card: UploadCard): void {
  placeholderOrder += 1;
  const meta: UploadMeta = { add: { id: card.id, pos, element: card.element, order: placeholderOrder } };
  view.dispatch(view.state.tr.setMeta(attachmentUploadsKey, meta));
}

function removePlaceholder(view: EditorView, id: string): void {
  const meta: UploadMeta = { remove: id };
  view.dispatch(view.state.tr.setMeta(attachmentUploadsKey, meta));
}

function placeholderPos(state: EditorState, id: string): number | null {
  const set = attachmentUploadsKey.getState(state);
  const found = set?.find(undefined, undefined, (spec) => (spec as Partial<PlaceholderSpec>).uploadId === id)[0];
  return found === undefined ? null : found.from;
}

/** The ids of the placeholders an editor shows, in document order (for tests). */
export function placeholderIds(state: EditorState): string[] {
  const set = attachmentUploadsKey.getState(state);
  return (set?.find() ?? []).map((decoration) => String((decoration.spec as Partial<PlaceholderSpec>).uploadId));
}

/** What several uploads to one place share: the empty paragraph the first block replaces. */
interface UploadGroup {
  replace: string | null;
}

/** Puts the block for an uploaded attachment where its placeholder is, and drops the placeholder. */
function insertBlock(
  view: EditorView,
  placeholder: string,
  kind: AttachmentKind,
  attachmentId: string,
  group: UploadGroup,
  extra: Record<string, unknown> = {},
): void {
  const { state } = view;
  const pos = placeholderPos(state, placeholder);
  const remove: UploadMeta = { remove: placeholder };
  const tr = state.tr.setMeta(attachmentUploadsKey, remove);
  const type = state.schema.nodes[kind === "image" ? IMAGE_NODE : FILE_NODE];
  if (pos !== null && type !== undefined) {
    const at = canInsertAt(tr.doc, pos, type) ? pos : placementFor(tr.doc, pos, type).pos;
    const node = type.create({ attachmentId, ...extra });
    tr.insert(at, node);
    const after = at + node.nodeSize;
    const next = tr.doc.nodeAt(after);
    if (group.replace !== null && next?.type.name === "paragraph" && next.content.size === 0 && next.attrs[BLOCK_ID_ATTRIBUTE] === group.replace) {
      const $after = tr.doc.resolve(after);
      if ($after.parent.canReplace($after.index(), $after.index() + 1)) {
        tr.delete(after, after + next.nodeSize);
      }
    }
    group.replace = null;
  }
  view.dispatch(tr);
}

/** `details.limit` of a size error, in bytes. */
function limitOf(details: unknown): number | null {
  if (typeof details !== "object" || details === null || Array.isArray(details)) {
    return null;
  }
  const limit: unknown = (details as Record<string, unknown>).limit;
  return typeof limit === "number" ? limit : null;
}

/** The message of a refused upload, for the card it leaves. */
export function describeUploadError(error: unknown, name: string): string {
  const failure = toCoreError(error);
  if (failure.code === "InvalidInput") {
    switch (detailOf(failure, "field")) {
      case "size":
        return t("attachment.error.size", { name, limit: formatBytes(limitOf(failure.details) ?? MAX_ATTACHMENT_BYTES) });
      case "name":
      case "mime":
        return t("attachment.error.refused", { name, message: failure.message });
    }
  }
  return t("attachment.error.upload", { name, message: failure.message });
}

/**
 * The message of a picture that could not be downloaded from a link, for the card it leaves. The
 * shell says why in `details.reason` (`status` with its code in `details.status`, `network`,
 * `timeout`, `redirect`, `notImage`) or `details.field` (`size`, with `details.limit`); a link it
 * refuses outright (not https, credentials, too long) comes with its own message. Nothing here
 * repeats the link: a link may carry a token.
 */
export function describeUrlError(error: unknown): string {
  const failure = toCoreError(error);
  if (failure.code === "InvalidInput") {
    const field = detailOf(failure, "field");
    if (field === "size") {
      return t("attachment.url.error.size", { limit: formatBytes(limitOf(failure.details) ?? MAX_PREVIEW_BYTES) });
    }
    if (field === "url") {
      switch (detailOf(failure, "reason")) {
        case "status":
          return t("attachment.url.error.status", { status: statusOf(failure.details) ?? "?" });
        case "network":
          return t("attachment.url.error.network");
        case "timeout":
          return t("attachment.url.error.timeout");
        case "redirect":
          return t("attachment.url.error.redirect");
        case "notImage":
          return t("attachment.url.error.notImage");
        default:
          return t("attachment.url.error.rejected", { message: failure.message });
      }
    }
  }
  return t("attachment.url.error.generic", { message: failure.message });
}

/** `details.status` of a failed download: the HTTP code. */
function statusOf(details: unknown): number | null {
  if (typeof details !== "object" || details === null || Array.isArray(details)) {
    return null;
  }
  const status: unknown = (details as Record<string, unknown>).status;
  return typeof status === "number" ? status : null;
}

/** The kind of block a file becomes: an image for `image/*`, a file card otherwise. */
export function kindForMime(mime: string): AttachmentKind {
  return mime.toLowerCase().startsWith("image/") ? "image" : "file";
}

/** What changes how uploaded files become blocks. */
export interface UploadOptions {
  /** A PDF becomes a file block that shows as a preview, not a card (the `/` command "PDF"). */
  preview?: boolean;
}

/**
 * Uploads files (pasted or dropped) one after the other and inserts a block for each at
 * `pos`, in order; a placeholder shows each one's progress meanwhile. Without uploads (the
 * transport cannot), one card says so.
 */
export function uploadFiles(editor: Editor, files: readonly File[], pos: number, options: UploadOptions = {}): void {
  const service = serviceOf(editor);
  const imageType = editor.schema.nodes[IMAGE_NODE];
  if (files.length === 0 || imageType === undefined || editor.isDestroyed) {
    return;
  }
  const target = placementFor(editor.state.doc, pos, imageType);
  const { view } = editor;
  if (service === null || !service.canUpload) {
    const card = new UploadCard(files.map((file) => file.name).join(", "), t("attachment.unsupported"), null);
    addPlaceholder(view, target.pos, card);
    card.fail(t("attachment.unsupported"), () => {
      removePlaceholder(view, card.id);
    });
    return;
  }
  const group: UploadGroup = { replace: target.replace };
  const jobs = files.map((file) => {
    const card = new UploadCard(file.name, t("attachment.uploading", { name: file.name }), file.size);
    addPlaceholder(view, target.pos, card);
    return { file, card };
  });
  void (async () => {
    for (const { file, card } of jobs) {
      if (editor.isDestroyed) {
        return;
      }
      try {
        const info = await service.upload(file, (sent, total) => {
          card.progress(sent, total ?? file.size);
        });
        if (editor.isDestroyed) {
          return;
        }
        const kind = kindForMime(info.mime);
        // A PDF, video or audio file the person asked to read or play in the page shows as a preview
        // (ADR-0031, ADR-0032).
        const preview =
          options.preview === true && kind === "file" && (isPdfFile(info.name, info.mime) || mediaKindOf(info.mime) !== null);
        insertBlock(view, card.id, kind, info.attachmentId, group, preview ? { view: "preview" } : {});
      } catch (error) {
        if (editor.isDestroyed) {
          return;
        }
        card.fail(describeUploadError(error, file.name), () => {
          removePlaceholder(view, card.id);
        });
      }
    }
  })();
}

/**
 * The `/` commands "Image" and "File": removes the typed command, lets the user pick a file
 * in the shell's dialog and inserts its block where the command was. "Image" makes an image
 * block of an image and a file block of anything else; "File" always makes a file block.
 */
export function importAttachment(editor: Editor, range: Range, kind: AttachmentKind): void {
  const service = serviceOf(editor);
  const imageType = editor.schema.nodes[IMAGE_NODE];
  if (service === null || !service.canImport || imageType === undefined) {
    return;
  }
  editor.chain().focus().deleteRange(range).run();
  const target = placementFor(editor.state.doc, editor.state.selection.from, imageType);
  const { view } = editor;
  const card = new UploadCard(t("attachment.importing"), t("attachment.importing"), null);
  addPlaceholder(view, target.pos, card);
  service.importFile().then(
    (info) => {
      if (editor.isDestroyed) {
        return;
      }
      if (info === null) {
        removePlaceholder(view, card.id); // the dialog was cancelled
        return;
      }
      const block = kind === "image" ? kindForMime(info.mime) : "file";
      insertBlock(view, card.id, block, info.attachmentId, { replace: target.replace });
    },
    (error: unknown) => {
      if (!editor.isDestroyed) {
        card.fail(t("attachment.error.import", { message: toCoreError(error).message }), () => {
          removePlaceholder(view, card.id);
        });
      }
    },
  );
}

/**
 * Opens the webview's file chooser for one file of the types in `accept`; resolves the file, or
 * `null` when the person cancels. The shell's own picker has no type filter, so this one is a plain
 * `<input type="file">` (as the gallery's), which needs no dialog permission.
 */
function chooseFile(accept: string): Promise<File | null> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = accept;
    input.addEventListener("change", () => {
      resolve(input.files?.[0] ?? null);
    });
    input.addEventListener("cancel", () => {
      resolve(null);
    });
    input.click();
  });
}

/**
 * The `/` command "PDF" (ADR-0031): removes the typed command, lets the person pick a PDF with
 * the webview's chooser, uploads it like a dropped file and inserts a file block that shows it as a
 * preview (a file that turns out not to be a PDF becomes the usual file card).
 */
export function insertPdf(editor: Editor, range: Range): void {
  const service = serviceOf(editor);
  if (service === null || !service.canUpload || editor.isDestroyed) {
    return;
  }
  editor.chain().focus().deleteRange(range).run();
  void chooseFile("application/pdf,.pdf").then((file) => {
    if (file !== null && !editor.isDestroyed) {
      uploadFiles(editor, [file], editor.state.selection.from, { preview: true });
    }
  });
}

/** What the chooser of `/video` and `/audio` lists (the formats the webviews play, and any other of the kind). */
export const VIDEO_ACCEPT = "video/*,.mp4,.m4v,.webm,.ogv,.mov";
export const AUDIO_ACCEPT = "audio/*,.mp3,.m4a,.aac,.ogg,.oga,.opus,.wav,.flac";

/**
 * The `/` commands "Video" and "Audio" (ADR-0032): as {@link insertPdf}, with a chooser restricted
 * to videos or to sounds; the file block shows a player. A file that turns out not to be of the
 * kind (the chooser can be overridden) becomes the usual file card or image.
 */
export function insertMedia(editor: Editor, range: Range, kind: "video" | "audio"): void {
  const service = serviceOf(editor);
  if (service === null || !service.canUpload || editor.isDestroyed) {
    return;
  }
  editor.chain().focus().deleteRange(range).run();
  void chooseFile(kind === "video" ? VIDEO_ACCEPT : AUDIO_ACCEPT).then((file) => {
    if (file !== null && !editor.isDestroyed) {
      uploadFiles(editor, [file], editor.state.selection.from, { preview: true });
    }
  });
}

/**
 * The `/` command "Image from URL" (ADR-0028): removes the typed command and leaves a card where
 * the block will go, kept out of sight while the person is asked for the link (a dialog, so that
 * the place is held however the page changes meanwhile). A cancelled dialog takes the card away.
 * A link makes the card say the picture is downloading, as an import does for a file, while the
 * shell fetches it once; then an `image` block of the new attachment stands in its place, and the
 * page holds the attachment only, never the link. A failure leaves the usual error card, saying
 * why ({@link describeUrlError}), until it is dismissed.
 */
export function importImageFromUrl(editor: Editor, range: Range): void {
  const service = serviceOf(editor);
  const imageType = editor.schema.nodes[IMAGE_NODE];
  if (service === null || !service.canImportUrl || imageType === undefined) {
    return;
  }
  editor.chain().focus().deleteRange(range).run();
  const target = placementFor(editor.state.doc, editor.state.selection.from, imageType);
  const { view } = editor;
  // Titled like the dialog; the status line says it downloads, then why it failed.
  const card = new UploadCard(t("attachment.url.title"), t("attachment.url.downloading"), null);
  card.note(t("attachment.url.downloading"));
  card.conceal();
  addPlaceholder(view, target.pos, card);
  /** Back to the page, the caret where it was. */
  const returnToPage = (): void => {
    if (!editor.isDestroyed) {
      view.focus();
    }
  };
  const dismiss = (): void => {
    removePlaceholder(view, card.id);
  };
  service.askUrl().then(
    (url) => {
      if (editor.isDestroyed) {
        return;
      }
      if (url === null) {
        dismiss(); // cancelled
        returnToPage();
        return;
      }
      card.reveal();
      returnToPage();
      service.importUrl(url).then(
        (info) => {
          if (!editor.isDestroyed) {
            insertBlock(view, card.id, kindForMime(info.mime), info.attachmentId, { replace: target.replace });
          }
        },
        (error: unknown) => {
          if (!editor.isDestroyed) {
            card.fail(describeUrlError(error), dismiss);
          }
        },
      );
    },
    (error: unknown) => {
      // `askUrl` does not reject; if it ever does, the card says so rather than staying hidden.
      if (!editor.isDestroyed) {
        card.reveal();
        card.fail(describeUrlError(error), dismiss);
      }
    },
  );
}

/** The files a paste or a drop carries. */
function filesOf(data: DataTransfer | null): File[] {
  // `files` is missing on some synthetic events (and on data built by other editors' scripts).
  const files: FileList | null | undefined = data?.files;
  return files === undefined || files === null ? [] : Array.from(files);
}

/**
 * The block id of the gallery that takes the pictures of a paste or a drop (ADR-0030), or `null`
 * (they become image blocks of the page): a paste goes to the gallery that is selected, a drop to
 * the gallery under the pointer (its view carries `data-gallery-block`).
 */
function galleryTarget(view: EditorView, target: EventTarget | null): string | null {
  if (target === null) {
    const { selection } = view.state;
    const id: unknown = selection instanceof NodeSelection ? selection.node.attrs[BLOCK_ID_ATTRIBUTE] : null;
    return selection instanceof NodeSelection && selection.node.type.name === GALLERY_NODE && typeof id === "string" ? id : null;
  }
  const block = target instanceof Element ? target.closest("[data-gallery-block]") : null;
  const id = block?.getAttribute("data-gallery-block");
  return id === undefined || id === null || id === "" ? null : id;
}

/** Where a drop lands in the document: under the pointer, else at the selection. */
function dropPosition(view: EditorView, event: DragEvent): number {
  try {
    const found = view.posAtCoords({ left: event.clientX, top: event.clientY });
    if (found !== null) {
      return found.pos;
    }
  } catch {
    // No layout to measure (tests): the selection decides.
  }
  return view.state.selection.from;
}

/**
 * Uploads, paste and drop in the editor: placeholders as decorations, image data pasted from
 * the clipboard and files dropped on the page become attachment blocks.
 */
export const Attachments = Extension.create<AttachmentsOptions, AttachmentsStorage>({
  name: "attachments",

  addOptions() {
    return { service: null };
  },

  addStorage() {
    return { service: this.options.service };
  },

  addProseMirrorPlugins() {
    const { editor } = this;
    return [
      new Plugin<DecorationSet>({
        key: attachmentUploadsKey,
        state: {
          init: () => DecorationSet.empty,
          apply: (tr, decorations) => {
            let next = decorations.map(tr.mapping, tr.doc);
            const meta = tr.getMeta(attachmentUploadsKey) as UploadMeta | undefined;
            if (meta !== undefined && "add" in meta) {
              const { id, pos, element, order } = meta.add;
              const spec: PlaceholderSpec & { key: string; side: number; ignoreSelection: boolean; stopEvent: () => boolean } = {
                uploadId: id,
                key: id,
                side: order, // after earlier placeholders at the same place; maps forward over insertions there
                ignoreSelection: true,
                stopEvent: () => true, // the card's own button
              };
              next = next.add(tr.doc, [Decoration.widget(Math.min(pos, tr.doc.content.size), element, spec)]);
            } else if (meta !== undefined && "remove" in meta) {
              next = next.remove(next.find(undefined, undefined, (spec) => (spec as Partial<PlaceholderSpec>).uploadId === meta.remove));
            }
            return next;
          },
        },
        props: {
          decorations: (state) => attachmentUploadsKey.getState(state),
          handlePaste: (view, event) => {
            const files = filesOf(event.clipboardData);
            // Text that comes with a picture of itself (a spreadsheet's cells) pastes as text.
            if (files.length === 0 || (event.clipboardData?.getData("text/plain") ?? "") !== "" || !view.editable) {
              return false;
            }
            event.preventDefault();
            const gallery = galleryTarget(view, null);
            if (gallery !== null && canAddToGallery(editor)) {
              addImagesToGallery(editor, gallery, files);
            } else {
              uploadFiles(editor, files, view.state.selection.from);
            }
            return true;
          },
          handleDOMEvents: {
            drop: (view, event) => {
              const files = filesOf(event.dataTransfer);
              if (files.length === 0 || view.dragging !== null) {
                return false; // moving blocks is ProseMirror's own drop
              }
              // Never let the webview open a dropped file in place of the app.
              event.preventDefault();
              if (view.editable) {
                const gallery = galleryTarget(view, event.target);
                if (gallery !== null && canAddToGallery(editor)) {
                  addImagesToGallery(editor, gallery, files);
                } else {
                  uploadFiles(editor, files, dropPosition(view, event));
                }
              }
              return true;
            },
          },
        },
      }),
    ];
  },
});
