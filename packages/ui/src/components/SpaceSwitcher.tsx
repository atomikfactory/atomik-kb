import { toCoreError } from "@atomik-kb/core-client";
import type { PageNode, SpaceId, SpaceInfo } from "@atomik-kb/core-client";
import { useId, useLayoutEffect, useRef, useState } from "react";
import type { DragEvent, FormEvent } from "react";

import { isFieldError } from "../errors";
import { t, tp } from "../i18n";
import { useCurrentAccess } from "../sharing-hooks";
import { useAppStore } from "../store-context";
import { selectCurrentSpace } from "../store-structure";
import { useRunAction } from "./actions";
import { ConfirmDialog } from "./ConfirmDialog";
import { ExportDialog } from "./ExportDialog";
import { FloatingPanel, anchorBelow } from "./FloatingPanel";
import type { Anchor } from "./FloatingPanel";
import { ChevronDownIcon, MoreIcon } from "./icons";
import { IconPicker } from "./IconPicker";
import { ImportDialog } from "./ImportDialog";
import { InlineEdit } from "./InlineEdit";
import { Menu } from "./Menu";
import type { MenuItem } from "./Menu";
import { MembersDialog } from "./MembersDialog";
import { PageIcon } from "./PageIcon";
import { SpaceAvatars } from "./PresenceAvatars";
import { PropertiesDialog } from "./PropertiesDialog";
import { ShareDialog } from "./ShareDialog";
import { SharedBadge } from "./SharedBadge";

function describeCreateError(failure: unknown): string {
  const error = toCoreError(failure);
  return isFieldError(error, "name")
    ? t("form.fieldError", { message: error.message })
    : t("spaces.error.create", { message: error.message });
}

/** Asks for a name and creates the space (which then becomes the current one). */
function NewSpaceForm({ onDone }: { onDone?: () => void }) {
  const createSpace = useAppStore((state) => state.createSpace);
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const inputId = useId();
  const errorId = useId();

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    if (name.trim() === "") {
      setError(t("spaces.error.empty"));
      return;
    }
    setError(null);
    setBusy(true);
    try {
      await createSpace(name);
      onDone?.();
    } catch (failure) {
      setError(describeCreateError(failure));
      setBusy(false);
    }
  }

  return (
    <form
      className="space-form"
      noValidate
      onSubmit={(event) => {
        void submit(event);
      }}
    >
      <label htmlFor={inputId}>{t("spaces.nameLabel")}</label>
      <div className="row">
        <input
          id={inputId}
          className="input"
          type="text"
          value={name}
          placeholder={t("spaces.namePlaceholder")}
          autoComplete="off"
          autoFocus={onDone !== undefined}
          aria-invalid={error !== null}
          aria-describedby={error !== null ? errorId : undefined}
          onChange={(event) => {
            setName(event.target.value);
            setError(null);
          }}
        />
        <button type="submit" className="button button-primary" disabled={busy}>
          {busy ? t("spaces.creating") : t("spaces.create")}
        </button>
      </div>
      {error !== null && (
        <p className="error-text" id={errorId} role="alert">
          {error}
        </p>
      )}
      {onDone !== undefined && (
        <button type="button" className="link-button" onClick={onDone}>
          {t("spaces.cancel")}
        </button>
      )}
    </form>
  );
}

/** The part of a list item the pointer is over: the upper half is "before", the lower "after". */
function halfOf(event: DragEvent<HTMLElement>): "before" | "after" {
  const rect = event.currentTarget.getBoundingClientRect();
  return rect.height > 0 && event.clientY - rect.top < rect.height / 2 ? "before" : "after";
}

/** The spaces to pick from, reorderable by drag and drop. */
function SpaceList({ spaces, currentId, onPick }: { spaces: readonly SpaceInfo[]; currentId: SpaceId | null; onPick: (id: SpaceId) => void }) {
  const run = useRunAction();
  const moveSpace = useAppStore((state) => state.moveSpace);
  const [dragId, setDragId] = useState<SpaceId | null>(null);
  const [hint, setHint] = useState<{ id: SpaceId; zone: "before" | "after" } | null>(null);

  function dropOn(event: DragEvent<HTMLElement>, targetId: SpaceId): void {
    event.preventDefault();
    const zone = halfOf(event);
    const id = dragId;
    setDragId(null);
    setHint(null);
    if (id === null || id === targetId) {
      return;
    }
    const others = spaces.filter((space) => space.id !== id);
    const at = others.findIndex((space) => space.id === targetId);
    const index = zone === "before" ? at : at + 1;
    if (index !== spaces.findIndex((space) => space.id === id)) {
      void run(moveSpace(id, index), "spaces.error.move");
    }
  }

  return (
    <ul className="space-list" aria-label={t("spaces.listLabel")}>
      {spaces.map((space) => {
        const zone = hint?.id === space.id ? hint.zone : null;
        const classes = ["space-row"];
        if (space.id === dragId) {
          classes.push("is-dragging");
        }
        if (zone !== null) {
          classes.push(`drop-${zone}`);
        }
        return (
          <li
            key={space.id}
            className={classes.join(" ")}
            draggable
            onDragStart={(event) => {
              event.dataTransfer.setData("application/x-atomik-space", space.id);
              event.dataTransfer.effectAllowed = "move";
              setDragId(space.id);
            }}
            onDragOver={(event) => {
              if (dragId === null || dragId === space.id) {
                return;
              }
              event.preventDefault();
              event.dataTransfer.dropEffect = "move";
              const next = halfOf(event);
              setHint((current) => (current?.id === space.id && current.zone === next ? current : { id: space.id, zone: next }));
            }}
            onDragLeave={(event) => {
              const into = event.relatedTarget;
              if (!(into instanceof Node && event.currentTarget.contains(into))) {
                setHint((current) => (current?.id === space.id ? null : current));
              }
            }}
            onDrop={(event) => {
              dropOn(event, space.id);
            }}
            onDragEnd={() => {
              setDragId(null);
              setHint(null);
            }}
          >
            <button
              type="button"
              className="space-item"
              aria-current={space.id === currentId ? "true" : undefined}
              onClick={() => {
                onPick(space.id);
              }}
            >
              <PageIcon icon={space.icon} kind="space" className="tree-icon" />
              <span className="space-item-name" title={space.name}>
                {space.name}
              </span>
              <SharedBadge space={space} />
              {space.shareState === "Shared" && <SpaceAvatars spaceId={space.id} />}
            </button>
          </li>
        );
      })}
    </ul>
  );
}


interface Opened {
  anchor: Anchor;
  /** The button that opened it, so pressing it again closes instead of reopening. */
  trigger: Element;
}

/** A page and everything below it. */
function countNodes(node: PageNode): number {
  return node.children.reduce((total, child) => total + countNodes(child), 1);
}

/**
 * The top of the sidebar: the current space (icon and name) that opens the list of
 * spaces to switch between, create and reorder, and a menu to rename it, change its
 * icon, manage its properties, sort its top-level pages (ADR-0052), export it or import into it (ADR-0036), import an AppFlowy
 * workspace into new spaces (ADR-0039), move it or delete it.
 * A workspace without spaces asks for the first one here.
 *
 * Shared spaces (Milestone 13): a personal space can be shared ("Share this space...", `ShareDialog`); a shared one opens its
 * member panel ("Members and sharing...", `MembersDialog`) and is left or closed instead of deleted; a read-only copy (a closed
 * space, or one this account was removed from) is removed from this device. A role that cannot edit gets no rename, icon,
 * properties or import. The list ends with "Join a shared space...", and a shared space shows its member count beside its name.
 */
export function SpaceSwitcher() {
  const run = useRunAction();
  const spaces = useAppStore((state) => state.spaces);
  const spacesLoaded = useAppStore((state) => state.spacesLoaded);
  const current = useAppStore(selectCurrentSpace);
  const tree = useAppStore((state) => state.tree);
  const selectSpace = useAppStore((state) => state.selectSpace);
  const renameSpace = useAppStore((state) => state.renameSpace);
  const setSpaceIcon = useAppStore((state) => state.setSpaceIcon);
  const moveSpace = useAppStore((state) => state.moveSpace);
  const sortChildren = useAppStore((state) => state.sortChildren);
  const deleteSpace = useAppStore((state) => state.deleteSpace);
  const interop = useAppStore((state) => state.interopCapabilities);
  const access = useCurrentAccess();
  const openJoinDialog = useAppStore((state) => state.openJoinDialog);
  const leaveSpace = useAppStore((state) => state.leaveSpace);
  // The menu bar's "Import..." and, with no page open, "Export..." (ADR-0040) ask for these dialogs.
  const requested = useAppStore((state) => (state.fileRequest?.target === "space" ? state.fileRequest.kind : null));
  const clearFileRequest = useAppStore((state) => state.clearFileRequest);

  const [list, setList] = useState<Opened | null>(null);
  const [adding, setAdding] = useState(false);
  const [menu, setMenu] = useState<Opened | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [iconPicker, setIconPicker] = useState<(Opened & { returnTo: Element | null }) | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState<{ returnTo: Element | null } | null>(null);
  const [properties, setProperties] = useState<{ returnTo: Element | null } | null>(null);
  const [transfer, setTransfer] = useState<{ kind: "export" | "import" | "appflowy"; returnTo: Element | null } | null>(null);
  // The space a sharing dialog was opened for stays its space when the current one changes (leaving a space makes another current).
  const [sharing, setSharing] = useState<{
    kind: "share" | "members";
    spaceId: SpaceId;
    name: string;
    start?: "code" | "link";
    returnTo: Element | null;
  } | null>(null);
  const [removingCopy, setRemovingCopy] = useState<{ returnTo: Element | null } | null>(null);
  const switcher = useRef<HTMLButtonElement>(null);
  const refocusSwitcher = useRef(false);

  useLayoutEffect(() => {
    if (refocusSwitcher.current && switcher.current !== null) {
      refocusSwitcher.current = false;
      if (document.activeElement === null || document.activeElement === document.body) {
        switcher.current.focus();
      }
    }
  });

  if (!spacesLoaded) {
    return null;
  }
  if (spaces.length === 0) {
    return (
      <div className="space-first">
        <p className="space-first-title">{t("spaces.firstTitle")}</p>
        <p className="muted">{t("spaces.firstHint")}</p>
        <NewSpaceForm />
      </div>
    );
  }

  const position = current === null ? -1 : spaces.findIndex((space) => space.id === current.id);
  // The pages a deletion takes (live and in the trash) are known for the current space only: that is the one whose
  // tree is loaded.
  const pagesAtStake =
    tree === null
      ? 0
      : tree.roots.reduce((total, node) => total + countNodes(node), 0) +
        tree.trashed.reduce((total, entry) => total + 1 + entry.descendantCount, 0);

  // A space is shared or personal, and a role may not edit: the menu offers what the access allows (Milestone 13).
  const optionItems: MenuItem[] = [
    ...(access.canEdit
      ? [
          { id: "rename", label: t("spaces.rename") },
          { id: "icon", label: t("spaces.changeIcon") },
          { id: "properties", label: t("properties.menu.space") },
          // One-shot sorts of the top-level pages (ADR-0052); they need two pages to put in order.
          { id: "sortTitle", label: t("spaces.sortTitle"), disabled: (tree?.roots.length ?? 0) < 2 },
          { id: "sortCreated", label: t("spaces.sortCreated"), disabled: (tree?.roots.length ?? 0) < 2 },
        ]
      : []),
    ...(!access.shared ? [{ id: "share", label: t("sharing.share.menu") }] : []),
    ...(access.shared ? [{ id: "members", label: t("sharing.menu.members") }] : []),
    ...(interop.export ? [{ id: "export", label: t("interop.menu.exportSpace") }] : []),
    ...(interop.import && access.canEdit ? [{ id: "import", label: t("interop.menu.importSpace") }] : []),
    ...(interop.import ? [{ id: "appflowy", label: t("appflowyImport.menu") }] : []),
    { id: "up", label: t("spaces.moveUp"), disabled: position <= 0 },
    { id: "down", label: t("spaces.moveDown"), disabled: position >= spaces.length - 1 },
    !access.shared
      ? {
          id: "delete",
          label: t("spaces.delete"),
          danger: true,
        }
      : access.archived
        ? { id: "removeCopy", label: t("sharing.archive.remove"), danger: true }
        : { id: "leaveClose", label: t("sharing.menu.leaveOrClose"), danger: true },
  ];

  function runOption(space: SpaceInfo, command: string): void {
    if (command === "rename") {
      setRenaming(true);
    } else if (command === "icon") {
      const button = switcher.current;
      if (button !== null) {
        setIconPicker({ anchor: anchorBelow(button), trigger: button, returnTo: menu?.trigger ?? null });
      }
    } else if (command === "properties") {
      setProperties({ returnTo: menu?.trigger ?? null });
    } else if (command === "sortTitle" || command === "sortCreated") {
      void run(sortChildren(space.id, null, command === "sortTitle" ? "title" : "created"), "spaces.error.sort");
    } else if (command === "export" || command === "import" || command === "appflowy") {
      setTransfer({ kind: command, returnTo: menu?.trigger ?? null });
    } else if (command === "up") {
      void run(moveSpace(space.id, position - 1), "spaces.error.move");
    } else if (command === "down") {
      void run(moveSpace(space.id, position + 1), "spaces.error.move");
    } else if (command === "delete") {
      setConfirmingDelete({ returnTo: menu?.trigger ?? null });
    } else if (command === "share") {
      setSharing({ kind: "share", spaceId: space.id, name: space.name, returnTo: menu?.trigger ?? null });
    } else if (command === "members" || command === "leaveClose") {
      setSharing({ kind: "members", spaceId: space.id, name: space.name, returnTo: menu?.trigger ?? null });
    } else if (command === "removeCopy") {
      setRemovingCopy({ returnTo: menu?.trigger ?? null });
    }
  }

  function closeList(): void {
    setList(null);
    setAdding(false);
  }

  return (
    <div className="space-switcher">
      {renaming && current !== null ? (
        <InlineEdit
          className="space-rename"
          value={current.name}
          label={t("spaces.nameLabel")}
          onCommit={(name) => run(renameSpace(current.id, name), "spaces.error.rename")}
          onDone={() => {
            refocusSwitcher.current = true;
            setRenaming(false);
          }}
        />
      ) : (
        <button
          ref={switcher}
          type="button"
          className="space-button"
          aria-haspopup="dialog"
          aria-expanded={list !== null}
          title={t("spaces.switchTitle")}
          onClick={(event) => {
            if (list === null) {
              setList({ anchor: anchorBelow(event.currentTarget), trigger: event.currentTarget });
            } else {
              closeList();
            }
          }}
        >
          <PageIcon icon={current?.icon} kind="space" className="tree-icon" />
          <span className="space-name" title={current?.name}>
            {current?.name ?? ""}
          </span>
          {current !== null && <SharedBadge space={current} />}
          {current?.shareState === "Shared" && <SpaceAvatars spaceId={current.id} />}
          <ChevronDownIcon />
        </button>
      )}
      {current !== null && (
        <button
          type="button"
          className="icon-button"
          aria-label={t("spaces.optionsLabel")}
          aria-haspopup="menu"
          onClick={(event) => {
            setMenu({ anchor: anchorBelow(event.currentTarget), trigger: event.currentTarget });
          }}
        >
          <MoreIcon />
        </button>
      )}

      {list !== null && (
        <FloatingPanel
          label={t("spaces.listLabel")}
          className="space-popover"
          anchor={list.anchor}
          anchorElement={list.trigger}
          onClose={closeList}
        >
          <SpaceList
            spaces={spaces}
            currentId={current?.id ?? null}
            onPick={(id) => {
              selectSpace(id);
              closeList();
            }}
          />
          {adding ? (
            <NewSpaceForm onDone={closeList} />
          ) : (
            <>
              <button
                type="button"
                className="menu-item"
                onClick={() => {
                  setAdding(true);
                }}
              >
                {t("spaces.newButton")}
              </button>
              <button
                type="button"
                className="menu-item"
                onClick={() => {
                  closeList();
                  openJoinDialog();
                }}
              >
                {t("sharing.menu.join")}
              </button>
            </>
          )}
        </FloatingPanel>
      )}
      {menu !== null && current !== null && (
        <Menu
          label={t("spaces.optionsLabel")}
          items={optionItems}
          anchor={menu.anchor}
          anchorElement={menu.trigger}
          onSelect={(command) => {
            runOption(current, command);
          }}
          onClose={() => {
            setMenu(null);
          }}
        />
      )}
      {iconPicker !== null && current !== null && (
        <IconPicker
          anchor={iconPicker.anchor}
          anchorElement={iconPicker.trigger}
          returnFocus={iconPicker.returnTo}
          current={current.icon}
          allowUpload={false}
          onSelect={(icon) => setSpaceIcon(current.id, icon)}
          onClose={() => {
            setIconPicker(null);
          }}
        />
      )}
      {properties !== null && current !== null && (
        <PropertiesDialog
          spaceId={current.id}
          returnFocus={properties.returnTo}
          onClose={() => {
            setProperties(null);
          }}
        />
      )}
      {(transfer?.kind === "export" || requested === "export") && current !== null && (
        <ExportDialog
          spaceId={current.id}
          name={current.name}
          returnFocus={transfer?.returnTo ?? null}
          onClose={() => {
            setTransfer(null);
            clearFileRequest();
          }}
        />
      )}
      {(transfer?.kind === "import" || transfer?.kind === "appflowy" || requested === "import") && current !== null && (
        <ImportDialog
          spaceId={current.id}
          destination={current.name}
          appflowy={transfer?.kind === "appflowy"}
          returnFocus={transfer?.returnTo ?? null}
          onClose={() => {
            setTransfer(null);
            clearFileRequest();
          }}
        />
      )}
      {sharing?.kind === "share" && (
        <ShareDialog
          spaceId={sharing.spaceId}
          name={sharing.name}
          returnFocus={sharing.returnTo}
          onInvite={() => {
            setSharing({ ...sharing, kind: "members", start: "code" });
          }}
          onClose={() => {
            setSharing(null);
          }}
        />
      )}
      {sharing?.kind === "members" && (
        <MembersDialog
          spaceId={sharing.spaceId}
          returnFocus={sharing.returnTo}
          start={sharing.start}
          onClose={() => {
            setSharing(null);
          }}
        />
      )}
      {removingCopy !== null && current !== null && (
        <ConfirmDialog
          returnFocus={removingCopy.returnTo}
          title={t("sharing.archive.removeTitle", { name: current.name })}
          body={t("sharing.archive.removeBody")}
          confirmLabel={t("sharing.archive.removeConfirm")}
          onConfirm={() => run(leaveSpace(current.id), "sharing.archive.error")}
          onClose={() => {
            setRemovingCopy(null);
          }}
        />
      )}
      {confirmingDelete !== null && current !== null && (
        <ConfirmDialog
          returnFocus={confirmingDelete.returnTo}
          title={t("spaces.deleteTitle")}
          body={
            pagesAtStake === 0
              ? t("spaces.deleteBody", { name: current.name })
              : tp("spaces.deleteBodyPages", pagesAtStake, { name: current.name })
          }
          typeToConfirm={
            pagesAtStake === 0 ? undefined : { phrase: current.name, label: t("spaces.deleteTypeLabel") }
          }
          confirmLabel={t("spaces.deleteConfirm")}
          onConfirm={() => run(deleteSpace(current.id), "spaces.error.delete")}
          onClose={() => {
            setConfirmingDelete(null);
          }}
        />
      )}
    </div>
  );
}
