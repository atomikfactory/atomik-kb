// The page the Android app asks for: its title and its editor, over a writable store whose navigation is the app's. Nothing of
// the desktop shell is here (no sidebar, tabs, menus); the app owns those and this page only tells it what happened.

import { CoreClient } from "@atomik-kb/core-client";
import type { PageInfo, SpaceInfo } from "@atomik-kb/core-client";
import type { AndroidTransport } from "@atomik-kb/core-client/android";
import { createAppStore, loadLanguage, PageEditor, StoreContext } from "@atomik-kb/ui/mobile";
import type { AppStore } from "@atomik-kb/ui/mobile";
import type { Editor } from "@tiptap/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { parseIncoming } from "./host";
import type { DictationInfo, InitInfo, InsertRequest, Theme } from "./host";
import { insertInto } from "./insert";
import { useKeyboardInset } from "./keyboard";
import { Toolbar } from "./Toolbar";

interface MobileAppProps {
  transport: AndroidTransport;
}

function applyTheme(theme: Theme): void {
  document.documentElement.setAttribute("data-theme", theme);
  document.documentElement.style.colorScheme = theme;
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** The space row of a page whose list could not be read: the editor needs one to know it may write. */
function fallbackSpace(spaceId: string, name: string): SpaceInfo {
  return { id: spaceId, name, createdAt: 0, pageCount: 1 };
}

/** The store of this page: the app's, never started, with navigation handed to the Android app. */
function pageStore(client: CoreClient, transport: AndroidTransport): AppStore {
  const store = createAppStore(client);
  const open = (pageId: string | null | undefined) => {
    if (typeof pageId === "string" && pageId !== "") {
      transport.notify({ host: "openPage", pageId });
    }
  };
  // Taps on page links, mentions, sub-page cards and database rows all end in one of these two.
  store.setState({
    navigateTo: (target: { pageId: string }) => {
      open(target.pageId);
    },
    selectPage: open,
  });
  return store;
}

export function MobileApp({ transport }: MobileAppProps) {
  const client = useMemo(() => new CoreClient(transport), [transport]);
  const store = useMemo(() => pageStore(client, transport), [client, transport]);
  const [init, setInit] = useState<InitInfo | null>(null);
  const [dictation, setDictation] = useState<DictationInfo>({ state: "idle" });
  const [editor, setEditor] = useState<Editor | null>(null);
  const [typing, setTyping] = useState(false);
  const pendingInserts = useRef<InsertRequest[]>([]);
  const editorRef = useRef<Editor | null>(null);

  const flushInserts = useCallback(() => {
    const target = editorRef.current;
    if (target === null || target.isDestroyed) {
      return;
    }
    for (const request of pendingInserts.current.splice(0)) {
      insertInto(target, request);
    }
  }, []);

  const handleEditor = useCallback(
    (next: Editor | null) => {
      editorRef.current = next;
      setEditor(next);
      flushInserts();
    },
    [flushInserts],
  );

  useEffect(() => {
    const stop = transport.onHostMessage((raw) => {
      const message = parseIncoming(raw);
      if (message === null) {
        return;
      }
      switch (message.kind) {
        case "init":
          applyTheme(message.init.theme);
          setInit((current) =>
            current !== null && current.pageId === message.init.pageId && current.spaceId === message.init.spaceId
              ? current
              : message.init,
          );
          break;
        case "theme":
          applyTheme(message.theme);
          break;
        case "dictation":
          setDictation(message.dictation);
          break;
        case "insert":
          pendingInserts.current.push(message.insert);
          flushInserts();
          break;
      }
    });
    const onError = (event: ErrorEvent) => {
      transport.notify({ host: "error", message: event.message });
    };
    const onRejection = (event: PromiseRejectionEvent) => {
      transport.notify({ host: "error", message: messageOf(event.reason) });
    };
    window.addEventListener("error", onError);
    window.addEventListener("unhandledrejection", onRejection);
    transport.notify({ host: "ready" });
    return () => {
      stop();
      window.removeEventListener("error", onError);
      window.removeEventListener("unhandledrejection", onRejection);
    };
  }, [transport, flushInserts]);

  // The editor and the title are what the toolbar is for.
  useEffect(() => {
    const check = () => {
      const active = document.activeElement;
      setTyping(
        active instanceof HTMLElement && (active.isContentEditable || active.classList.contains("mobile-title-input")),
      );
    };
    const afterFocusMoved = () => {
      setTimeout(check, 0);
    };
    document.addEventListener("focusin", check);
    document.addEventListener("focusout", afterFocusMoved);
    return () => {
      document.removeEventListener("focusin", check);
      document.removeEventListener("focusout", afterFocusMoved);
    };
  }, []);

  const inset = useKeyboardInset(
    useCallback(() => {
      // The keyboard just came up over the caret: bring it back into view.
      requestAnimationFrame(() => {
        const active = document.activeElement;
        if (active instanceof HTMLElement && active.isContentEditable) {
          const selection = window.getSelection();
          if (selection !== null && selection.rangeCount > 0) {
            const node = selection.getRangeAt(0).startContainer;
            (node instanceof Element ? node : node.parentElement)?.scrollIntoView({ block: "nearest" });
          }
        }
      });
    }, []),
  );

  if (init === null) {
    return null;
  }
  return (
    <StoreContext.Provider value={store}>
      <PageScreen
        key={`${init.spaceId}/${init.pageId}`}
        init={init}
        client={client}
        store={store}
        transport={transport}
        onEditor={handleEditor}
      />
      <Toolbar
        editor={editor}
        listening={dictation.state === "listening"}
        visible={typing || inset > 0}
        onDictate={() => {
          transport.notify({ host: "dictate" });
        }}
      />
      {dictation.state === "listening" && dictation.partial !== undefined && dictation.partial !== "" && (
        <p className="mobile-partial" role="status">
          {dictation.partial}
        </p>
      )}
    </StoreContext.Provider>
  );
}

interface PageScreenProps {
  init: InitInfo;
  client: CoreClient;
  store: AppStore;
  transport: AndroidTransport;
  onEditor: (editor: Editor | null) => void;
}

function PageScreen({ init, client, store, transport, onEditor }: PageScreenProps) {
  const [page, setPage] = useState<PageInfo | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [title, setTitle] = useState("");
  const titleRef = useRef<HTMLTextAreaElement>(null);
  const saved = useRef("");
  const editorRef = useRef<Editor | null>(null);

  useEffect(() => {
    let live = true;
    const load = async () => {
      const choice = init.locale.slice(0, 2);
      await loadLanguage(choice === "fr" || choice === "es" || choice === "en" ? choice : "system").catch(() => "en");
      const [spaces, info] = await Promise.all([
        client.spaceList().catch((): SpaceInfo[] => []),
        client.pageGet(init.pageId),
      ]);
      if (!live) {
        return;
      }
      const known = spaces.some((candidate) => candidate.id === init.spaceId);
      store.setState({ spaces: known ? spaces : [...spaces, fallbackSpace(init.spaceId, info.title)] });
      saved.current = info.title;
      setTitle(info.title);
      setPage(info);
      transport.notify({ host: "opened", pageId: info.id, title: info.title });
    };
    load().catch((error: unknown) => {
      if (live) {
        setFailure(messageOf(error));
        transport.notify({ host: "error", message: messageOf(error) });
      }
    });
    return () => {
      live = false;
    };
  }, [client, store, transport, init.pageId, init.spaceId, init.locale]);

  // The app or another device renamed the page while it is open.
  useEffect(
    () =>
      transport.subscribe((event) => {
        if (event.type !== "StructureChanged" || document.activeElement === titleRef.current) {
          return;
        }
        client
          .pageGet(init.pageId)
          .then((info) => {
            saved.current = info.title;
            setTitle(info.title);
          })
          .catch(() => undefined);
      }),
    [client, transport, init.pageId],
  );

  // The title grows with its text instead of scrolling inside one line.
  useEffect(() => {
    const area = titleRef.current;
    if (area !== null) {
      area.style.height = "auto";
      area.style.height = `${area.scrollHeight}px`;
    }
  }, [title, page]);

  const commit = useCallback(
    (next: string) => {
      const trimmed = next.trim();
      if (trimmed === saved.current) {
        return;
      }
      saved.current = trimmed;
      client
        .pageRename(init.pageId, trimmed)
        .then((info) => {
          transport.notify({ host: "titleChanged", title: info.title });
        })
        .catch((error: unknown) => {
          transport.notify({ host: "error", message: messageOf(error) });
        });
    },
    [client, transport, init.pageId],
  );

  // The title is saved a moment after the last key.
  useEffect(() => {
    if (page === null) {
      return undefined;
    }
    const timer = setTimeout(() => {
      commit(title);
    }, 700);
    return () => {
      clearTimeout(timer);
    };
  }, [title, page, commit]);

  const handleEditor = useCallback(
    (editor: Editor | null) => {
      editorRef.current = editor;
      onEditor(editor);
      if (editor !== null && !editor.isDestroyed && init.focus === "end") {
        editor.commands.focus("end");
      }
    },
    [onEditor, init.focus],
  );

  useEffect(() => {
    if (page !== null && init.focus === "title") {
      const area = titleRef.current;
      area?.focus();
      area?.setSelectionRange(area.value.length, area.value.length);
    }
  }, [page, init.focus]);

  if (failure !== null) {
    return (
      <p className="mobile-failure" role="alert">
        {failure}
      </p>
    );
  }
  if (page === null) {
    return null;
  }
  return (
    <div className="content mobile-content">
      <div className="content-body mobile-body">
        <section className="page mobile-page" aria-labelledby="page-title">
          <header className="page-header">
            <h1 className="title page-title mobile-title" id="page-title">
              <textarea
                ref={titleRef}
                className="mobile-title-input"
                rows={1}
                value={title}
                placeholder="Untitled"
                aria-label="Title"
                readOnly={init.readOnly}
                enterKeyHint="next"
                autoCapitalize="sentences"
                onChange={(event) => {
                  setTitle(event.target.value.replace(/\n/g, " "));
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    commit(title);
                    editorRef.current?.commands.focus("start");
                  }
                }}
                onBlur={() => {
                  commit(title);
                }}
              />
            </h1>
          </header>
          <PageEditor pageId={init.pageId} spaceId={init.spaceId} onEditor={handleEditor} locked={init.readOnly} />
        </section>
      </div>
    </div>
  );
}
