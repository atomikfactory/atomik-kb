// The viewer (M13 slice 4, ADR-0051): loads the link through its transport, shows the page read-only with
// `@atomik-kb/ui`'s page view, polls for a newer publication, and says so when the link is gone, the key does not
// open, or the server cannot be reached. A link that was revoked while the page is open replaces the page at the
// next poll: nothing of it stays on screen.

import { CoreClient } from "@atomik-kb/core-client";
import { ReadOnlyPageView } from "@atomik-kb/ui";
import { useEffect, useMemo, useState } from "react";

import { message } from "./messages";
import type { MessageKey, ViewerLanguage } from "./messages";
import { nextView } from "./view";
import type { View } from "./view";
import type { LinkLoad, ViewerTransport } from "./ViewerTransport";

const STATUS_KEYS: Readonly<Record<"gone" | "invalid" | "failed" | "nokey", { title: MessageKey; body: MessageKey }>> = {
  gone: { title: "gone.title", body: "gone.body" },
  invalid: { title: "invalid.title", body: "invalid.body" },
  failed: { title: "failed.title", body: "failed.body" },
  nokey: { title: "nokey.title", body: "nokey.body" },
};

/** A screen that is not the page. */
export function StatusScreen({
  kind,
  language,
  onRetry,
}: {
  kind: "gone" | "invalid" | "failed" | "nokey";
  language: ViewerLanguage;
  onRetry?: () => void;
}) {
  const keys = STATUS_KEYS[kind];
  return (
    <main className="viewer-status" role="alert">
      <h1>{message(language, keys.title)}</h1>
      <p>{message(language, keys.body)}</p>
      {kind === "failed" && onRetry !== undefined && (
        <button type="button" className="button" onClick={onRetry}>
          {message(language, "retry")}
        </button>
      )}
    </main>
  );
}

export interface ViewerProps {
  /** The transport of the link (its key is in it). */
  transport: ViewerTransport;
  /** The language of the viewer's own words. */
  language: ViewerLanguage;
}

/** The viewer of one link. */
export function Viewer({ transport, language }: ViewerProps) {
  const client = useMemo(() => new CoreClient(transport), [transport]);
  const [view, setView] = useState<View>({ kind: "loading" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let live = true;
    const apply = (load: LinkLoad): void => {
      if (live) {
        setView((current) => nextView(current, load));
      }
    };
    void transport.load().then(apply);
    const stop = transport.watch(apply);
    return () => {
      live = false;
      stop();
    };
  }, [transport, attempt]);

  const title = view.kind === "page" ? view.page.title : "";
  useEffect(() => {
    document.title = title === "" ? message(language, "title.fallback") : title;
  }, [title, language]);

  switch (view.kind) {
    case "loading":
      return (
        <p className="viewer-status" role="status">
          {message(language, "loading")}
        </p>
      );
    case "page":
      return (
        <div className="viewer-root">
          {view.offline && (
            <p className="viewer-note is-warning" role="status">
              {message(language, "offline")}
            </p>
          )}
          <p className="viewer-note">{message(language, "readOnly")}</p>
          <ReadOnlyPageView client={client} page={view.page} revision={view.revision} />
        </div>
      );
    default:
      return (
        <StatusScreen
          kind={view.kind}
          language={language}
          onRetry={() => {
            setView({ kind: "loading" });
            setAttempt((count) => count + 1);
          }}
        />
      );
  }
}
