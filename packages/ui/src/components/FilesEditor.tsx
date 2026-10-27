// The editor of a `files` property (ADR-0026, amendment of 2026-10-07): a row of chips, one per
// attachment, in the order of the value. A chip shows the attachment from the manifests the app
// already reads for the blocks of a page (an image as a thumbnail from its bytes, any other file
// with a glyph for its type and its name), and offers Open, Save as and Remove. "Add file" opens the
// shell's file picker and uploads the file into the page's space; where the shell has none, the
// button is disabled and says to drop the file instead (or that files cannot be added here), as the
// `/` commands of the editor do. Files dropped on the row are uploaded too. Every change is one
// `pagePropertySet` with the whole list; a refused file leaves the message under the row.

import { MAX_FILES, MAX_PREVIEW_BYTES, toCoreError } from "@atomik-kb/core-client";
import type { AttachmentId, AttachmentInfo } from "@atomik-kb/core-client";
import { useEffect, useRef, useState } from "react";
import type { DragEvent } from "react";

import { describeUploadError } from "../editor/attachments";
import { downloadedBytes, fileIcon, useAttachment, useImagePreview } from "../editor/AttachmentViews";
import { formatBytes } from "../format";
import { t } from "../i18n";
import { useAppStore } from "../store-context";
import { PlusIcon } from "./icons";
import { ErrorLine, useCommit } from "./property-editor-parts";
import type { PropertyEditorProps } from "./property-editor-parts";
import { CloseIcon } from "./property-icons";

/** The message that says why a file cannot be picked here, or `null` when it can. */
function pickerProblem(canImport: boolean, canUpload: boolean): string | null {
  if (canImport) {
    return null;
  }
  return canUpload ? t("slash.attachment.dropInstead") : t("slash.attachment.unsupported");
}

/** The line under the name of a chip: its size, or how far its transfer is. */
function detailOf(info: AttachmentInfo): string {
  switch (info.availability) {
    case "Complete":
      return formatBytes(info.size);
    case "Downloading":
      return t("attachment.transfer.downloading", {
        done: formatBytes(downloadedBytes(info)),
        total: formatBytes(info.size),
      });
    case "Waiting":
      return t("attachment.transfer.waiting");
    case "NotDownloaded":
      return t("attachment.transfer.notDownloaded", { size: formatBytes(info.size) });
  }
}

interface FileChipProps {
  attachmentId: AttachmentId;
  readOnly: boolean;
  onRemove: (attachmentId: AttachmentId) => void;
}

/** One attachment of the value. */
function FileChip({ attachmentId, readOnly, onRemove }: FileChipProps) {
  const lookup = useAttachment(attachmentId);
  const capabilities = useAppStore((state) => state.attachmentCapabilities);
  const saveAttachmentAs = useAppStore((state) => state.saveAttachmentAs);
  const openAttachment = useAppStore((state) => state.openAttachment);
  const downloadAttachment = useAppStore((state) => state.downloadAttachment);
  const [notice, setNotice] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [broken, setBroken] = useState<string | null>(null);
  const info = lookup.status === "found" ? lookup.info : null;
  const complete = info?.availability === "Complete";
  const isImage = info !== null && info.mime.toLowerCase().startsWith("image/");
  const previewable = isImage && complete && capabilities.read && Number(info.size) <= MAX_PREVIEW_BYTES;
  const preview = useImagePreview(previewable ? info : null);
  const thumbnail = preview?.status === "ready" && broken !== preview.url ? preview.url : null;

  async function run(action: () => Promise<unknown>, failure: "attachment.error.open" | "attachment.error.save"): Promise<void> {
    setNotice(null);
    setSaved(false);
    try {
      await action();
    } catch (error) {
      setNotice(t(failure, { message: toCoreError(error).message }));
    }
  }

  const name = info?.name ?? "";
  let label: string;
  let detail: string | null = null;
  switch (lookup.status) {
    case "loading":
      label = t("attachment.loading");
      break;
    case "missing":
      label = t("attachment.deleted.file");
      detail = t("attachment.deleted.detail");
      break;
    case "error":
      label = t("attachment.error.read", { message: lookup.message });
      break;
    case "found":
      label = lookup.info.name;
      detail = detailOf(lookup.info);
      break;
  }

  return (
    <li className="file-chip" data-state={lookup.status} data-kind={isImage ? "image" : "file"}>
      <span className="file-chip-thumb" aria-hidden={thumbnail === null ? "true" : undefined}>
        {thumbnail !== null ? (
          <img
            src={thumbnail}
            alt={t("properties.files.thumbnail", { name })}
            draggable={false}
            onError={() => {
              setBroken(thumbnail);
            }}
          />
        ) : (
          <span className="file-chip-glyph">{info === null ? "📎" : fileIcon(info.mime)}</span>
        )}
      </span>
      <span className="file-chip-text">
        <span className="file-chip-name" title={label}>
          {label}
        </span>
        {detail !== null && <span className="file-chip-detail muted">{detail}</span>}
      </span>
      <span className="file-chip-actions">
        {info !== null && info.availability === "NotDownloaded" && (
          <button
            type="button"
            className="button file-chip-button"
            onClick={() => {
              void run(() => downloadAttachment(info.attachmentId), "attachment.error.open");
            }}
          >
            {t("attachment.transfer.download")}
          </button>
        )}
        {info !== null && complete && capabilities.open && (
          <button
            type="button"
            className="button file-chip-button"
            aria-label={t("properties.files.open", { name })}
            onClick={() => {
              void run(() => openAttachment(info.attachmentId), "attachment.error.open");
            }}
          >
            {t("attachment.open")}
          </button>
        )}
        {info !== null && complete && capabilities.saveAs && (
          <button
            type="button"
            className="button file-chip-button"
            aria-label={t("properties.files.saveAs", { name })}
            onClick={() => {
              void run(async () => {
                setSaved(await saveAttachmentAs(info.attachmentId));
              }, "attachment.error.save");
            }}
          >
            {t("attachment.saveAs")}
          </button>
        )}
        {!readOnly && (
          <button
            type="button"
            className="icon-button file-chip-remove"
            aria-label={t("properties.files.remove", { name: info === null ? label : name })}
            title={t("properties.files.remove", { name: info === null ? label : name })}
            onClick={() => {
              onRemove(attachmentId);
            }}
          >
            <CloseIcon />
          </button>
        )}
      </span>
      {saved && (
        <span className="file-chip-note muted" role="status">
          {t("attachment.saved")}
        </span>
      )}
      {notice !== null && (
        <span className="file-chip-note error-text" role="alert">
          {notice}
        </span>
      )}
    </li>
  );
}

/** Whether a drag carries files (and not text or a block of the page). */
function carriesFiles(event: DragEvent): boolean {
  return Array.from(event.dataTransfer.types).includes("Files");
}

export function FilesEditor({ def, spaceId, value, readOnly, autoFocus, commit }: PropertyEditorProps) {
  const capabilities = useAppStore((state) => state.attachmentCapabilities);
  const importAttachment = useAppStore((state) => state.importAttachment);
  const uploadAttachment = useAppStore((state) => state.uploadAttachment);
  const { run, error, clearError } = useCommit(commit);
  const ids: readonly AttachmentId[] = value?.kind === "files" ? value.value : [];
  // What the value holds now, for the additions that finish after other changes were made.
  const current = useRef(ids);
  useEffect(() => {
    current.current = ids;
  });
  const [adding, setAdding] = useState(false);
  const [problems, setProblems] = useState<readonly string[]>([]);
  const [dragging, setDragging] = useState(false);
  const addButton = useRef<HTMLButtonElement>(null);

  // A property that was just added takes focus (its row is empty: the button is all there is).
  useEffect(() => {
    if (autoFocus) {
      addButton.current?.focus();
    }
  }, [autoFocus]);

  const problem = pickerProblem(capabilities.importPath, capabilities.upload);
  const full = ids.length >= MAX_FILES;

  /** Saves the value with `next` files (an empty list clears it). */
  function save(next: readonly AttachmentId[]): Promise<boolean> {
    return run(next.length === 0 ? null : { kind: "files", value: [...next] });
  }

  /** Appends attachments that are not in the value yet, up to the limit. */
  async function append(added: readonly AttachmentId[]): Promise<void> {
    const fresh = added.filter((id, at) => !current.current.includes(id) && added.indexOf(id) === at);
    if (fresh.length === 0) {
      return;
    }
    if (current.current.length + fresh.length > MAX_FILES) {
      setProblems((found) => [...found, t("properties.files.limit", { max: MAX_FILES })]);
      return;
    }
    await save([...current.current, ...fresh]);
  }

  async function pick(): Promise<void> {
    clearError();
    setProblems([]);
    setAdding(true);
    try {
      const info = await importAttachment(spaceId);
      if (info !== null) {
        await append([info.attachmentId]);
      }
    } catch (failure) {
      setProblems([t("attachment.error.import", { message: toCoreError(failure).message })]);
    }
    setAdding(false);
  }

  async function drop(files: readonly File[]): Promise<void> {
    clearError();
    setProblems([]);
    setAdding(true);
    const uploaded: AttachmentId[] = [];
    for (const file of files) {
      try {
        uploaded.push((await uploadAttachment(spaceId, file)).attachmentId);
      } catch (failure) {
        setProblems((found) => [...found, describeUploadError(failure, file.name)]);
      }
    }
    await append(uploaded);
    setAdding(false);
  }

  const droppable = !readOnly && capabilities.upload;
  const list =
    ids.length === 0 ? null : (
      <ul className="file-chips" aria-label={t("properties.files.label", { name: def.name })}>
        {ids.map((id) => (
          <FileChip
            key={id}
            attachmentId={id}
            readOnly={readOnly}
            onRemove={(removed) => {
              clearError();
              setProblems([]);
              void save(current.current.filter((entry) => entry !== removed));
            }}
          />
        ))}
      </ul>
    );

  if (readOnly) {
    return <div className="property-editor property-files">{list}</div>;
  }

  return (
    <div
      className={dragging ? "property-editor property-files is-dragging" : "property-editor property-files"}
      onDragOver={(event) => {
        if (droppable && carriesFiles(event)) {
          event.preventDefault();
          setDragging(true);
        }
      }}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
          setDragging(false);
        }
      }}
      onDrop={(event) => {
        setDragging(false);
        if (droppable && carriesFiles(event)) {
          // Never let the webview open a dropped file in place of the app.
          event.preventDefault();
          void drop(Array.from(event.dataTransfer.files));
        }
      }}
    >
      {list}
      <button
        ref={addButton}
        type="button"
        className={ids.length === 0 ? "property-chip is-empty" : "property-chip property-chip-add"}
        disabled={problem !== null || full || adding}
        aria-describedby={problem === null ? undefined : `${def.id}-files-hint`}
        onClick={() => {
          void pick();
        }}
      >
        <PlusIcon />
        <span>{adding ? t("properties.files.adding") : t("properties.files.add")}</span>
      </button>
      {problem !== null && (
        <span id={`${def.id}-files-hint`} className="muted properties-files-hint">
          {problem}
        </span>
      )}
      {full && problem === null && <span className="muted properties-files-hint">{t("properties.files.limit", { max: MAX_FILES })}</span>}
      {problems.map((message) => (
        <ErrorLine key={message} message={message} />
      ))}
      <ErrorLine message={error} />
    </div>
  );
}
