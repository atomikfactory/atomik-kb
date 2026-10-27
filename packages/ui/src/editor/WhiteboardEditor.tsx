// The full-screen editor of a whiteboard (ADR-0033): a modal over the whole window (the image and PDF
// viewers' pattern) that holds Excalidraw. It never outlives the unlocked app: the page editor goes
// with a lock, and this view with it, after writing what is still pending.
//
// The scene and the page document stay in step through `SceneSync` (whiteboard-sync.ts): the scene's
// changed elements are written to the document, a pause after the last change, one commit with the
// origin `whiteboard` (page undo does not rewind a drawing); what comes from the document (a sync
// import, another view of the block) is merged into the scene by version. Excalidraw's own undo and
// redo work inside the editor, on this session's scene.
//
// Nothing here reaches the network: the menu is our own (no links, no sign-in), the library, sharing,
// embeds, AI and images are off, links in the drawing do not open, and a drop or paste of a file is
// refused. The CSP is the app's.

import type { ExcalidrawImperativeAPI } from "@excalidraw/excalidraw/types";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { ClipboardEvent, DragEvent, MouseEvent, RefObject } from "react";

import { Modal } from "../components/Modal";
import { t } from "../i18n";
import { loadExcalidraw } from "./excalidraw-engine";
import type { ExcalidrawEngine } from "./excalidraw-engine";
import { useAppTheme } from "./WhiteboardPreview";
import { SceneSync } from "./whiteboard-sync";
import { textField } from "./whiteboard-sync";
import type { SceneElement } from "./whiteboard-sync";
import type { WhiteboardStore } from "./whiteboard-store";

/** Element types the editor does not keep: images (off in v1) and embeds. */
const HIDDEN_TYPES: ReadonlySet<string> = new Set(["image", "embeddable", "iframe"]);

function keepable(element: SceneElement): boolean {
  return !HIDDEN_TYPES.has(textField(element, "type"));
}

/**
 * The commands of Excalidraw's "More tools" menu this editor does not offer: embedding a web page (it
 * would load that page) and the Mermaid import (its converter is not part of the build, see
 * `apps/desktop/excalidraw-assets.ts`). The library has no option to leave them out of the menu, so
 * the menu's items are hidden as they appear.
 */
const HIDDEN_COMMANDS = /web embed|mermaid/iu;

export interface WhiteboardEditorProps {
  store: WhiteboardStore;
  blockId: string;
  /** Whether the drawing can be changed; otherwise the editor only looks. */
  editable: boolean;
  /** Gets focus back when the editor closes. */
  returnFocus: RefObject<HTMLElement | null>;
  onClose: () => void;
}

/** The modal editor of one drawing. */
export function WhiteboardEditor({ store, blockId, editable, returnFocus, onClose }: WhiteboardEditorProps) {
  const labelId = useId();
  const theme = useAppTheme();
  const [engine, setEngine] = useState<ExcalidrawEngine | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const apiRef = useRef<ExcalidrawImperativeAPI | null>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const sync = useMemo(() => new SceneSync(store, blockId), [store, blockId]);
  const initial = useMemo(() => sync.initial().filter(keepable), [sync]);

  useEffect(() => {
    let active = true;
    loadExcalidraw().then(
      (loaded) => {
        if (active) {
          setEngine(loaded);
        }
      },
      (error: unknown) => {
        if (active) {
          setFailure(error instanceof Error ? error.message : String(error));
        }
      },
    );
    return () => {
      active = false;
    };
  }, []);

  // Commands this editor does not offer are left out of the library's menus.
  useEffect(() => {
    const stage = stageRef.current;
    if (stage === null || typeof MutationObserver === "undefined") {
      return;
    }
    const hide = (): void => {
      for (const item of stage.querySelectorAll<HTMLElement>(".dropdown-menu-item")) {
        if (HIDDEN_COMMANDS.test(item.textContent ?? "")) {
          item.style.display = "none";
        }
      }
    };
    hide();
    const observer = new MutationObserver(hide);
    observer.observe(stage, { childList: true, subtree: true });
    return () => {
      observer.disconnect();
    };
  }, [engine]);

  // What reaches the document from elsewhere goes into the scene; our own writes come back here too and are no change.
  useEffect(() => {
    if (engine === null) {
      return;
    }
    return store.subscribe(blockId, () => {
      const api = apiRef.current;
      if (api === null) {
        return;
      }
      const { elements, changed } = sync.incoming(api.getSceneElementsIncludingDeleted() as unknown as SceneElement[]);
      if (changed) {
        api.updateScene({
          elements: engine.restoreElements(elements.filter(keepable)) as never,
          captureUpdate: engine.neverCapture,
        });
      }
    });
  }, [engine, store, blockId, sync]);

  // The pending changes are written when the editor goes, the window is hidden or the page is left.
  useEffect(() => {
    const flush = (): void => {
      sync.flush();
    };
    window.addEventListener("pagehide", flush);
    return () => {
      window.removeEventListener("pagehide", flush);
      sync.flush();
      sync.dispose();
    };
  }, [sync]);

  /** No file goes into the drawing, and no address opens from it. */
  function refuseFiles(event: DragEvent<HTMLElement> | ClipboardEvent<HTMLElement>): void {
    const transfer = "dataTransfer" in event ? event.dataTransfer : event.clipboardData;
    if (transfer.files.length > 0) {
      event.preventDefault();
      event.stopPropagation();
    }
  }

  function refuseLinks(event: MouseEvent<HTMLElement>): void {
    if (event.target instanceof Element && event.target.closest("a[href]") !== null) {
      event.preventDefault();
    }
  }

  function close(): void {
    sync.flush();
    onClose();
  }

  let body;
  if (failure !== null) {
    body = (
      <p className="whiteboard-modal-message" role="alert">
        {t("whiteboard.error", { message: failure })}
      </p>
    );
  } else if (engine === null) {
    body = (
      <p className="whiteboard-modal-message" role="status">
        {t("whiteboard.loading")}
      </p>
    );
  } else {
    const { Excalidraw, MainMenu } = engine;
    body = (
      <Excalidraw
        excalidrawAPI={(api) => {
          apiRef.current = api;
        }}
        initialData={{
          elements: engine.restoreElements(initial) as never,
          appState: { theme },
          scrollToContent: true,
        }}
        onChange={(elements) => {
          if (editable) {
            sync.sceneChanged(elements.filter((element) => keepable(element as unknown as SceneElement)) as unknown as SceneElement[]);
          }
        }}
        onPointerUp={() => {
          sync.flush();
        }}
        viewModeEnabled={!editable}
        theme={theme}
        name={t("whiteboard.title")}
        handleKeyboardGlobally={false}
        autoFocus
        aiEnabled={false}
        validateEmbeddable={false}
        onLinkOpen={(_element, event) => {
          event.preventDefault();
        }}
        onPaste={(_data, event) => (event?.clipboardData?.files.length ?? 0) === 0}
        UIOptions={{
          tools: { image: false },
          canvasActions: {
            loadScene: false,
            saveToActiveFile: false,
            export: false,
            saveAsImage: false,
            toggleTheme: false,
            clearCanvas: editable,
            changeViewBackgroundColor: editable,
          },
        }}
      >
        <MainMenu>
          <MainMenu.DefaultItems.ClearCanvas />
          <MainMenu.DefaultItems.ChangeCanvasBackground />
        </MainMenu>
      </Excalidraw>
    );
  }

  return (
    <Modal labelledBy={labelId} className="whiteboard-modal" returnFocus={returnFocus}>
      <div className="whiteboard-modal-bar">
        <h2 id={labelId} className="whiteboard-modal-title">
          {t("whiteboard.title")}
        </h2>
        <span className="whiteboard-modal-note">{editable ? t("whiteboard.saved") : t("whiteboard.readOnly")}</span>
        <button type="button" className="button" onClick={close}>
          {t("whiteboard.close")}
        </button>
      </div>
      <div
        ref={stageRef}
        className="whiteboard-modal-stage"
        onDropCapture={refuseFiles}
        onPasteCapture={refuseFiles}
        onClickCapture={refuseLinks}
        onAuxClickCapture={refuseLinks}
      >
        {body}
      </div>
    </Modal>
  );
}
