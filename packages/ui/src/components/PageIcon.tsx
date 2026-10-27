// The icon of a page or a space, wherever one is shown (ADR-0029, section 1): the sidebar, the
// breadcrumbs, links, search, relations, the list, table and board views. It reads the icon
// string the way the core stores it (`icons/icon-grammar.ts`): an emoji is text, `icon:<name>[:<color>]`
// is an icon of the bundled set in one of the palette's colours, `attachment:<id>` is an uploaded
// picture. Anything it cannot show, a name the set does not have, a colour it does not know, a
// picture that is missing, not downloaded yet or unreadable, is the default glyph of the kind
// (a page or a space), never the raw string.
//
// The component renders one `<span class="page-icon">` and nothing else; the place that shows it
// gives that span its size and colour through `className` (an emoji follows `font-size`, the icons
// and the picture are `1em` wide unless a rule says otherwise). It is decorative: the control or
// row around it carries the accessible name.

import { useContext, useState } from "react";
import type { ReactNode } from "react";

import { iconByName } from "../icons/icon-set";
import { useIconImage } from "../icons/icon-images";
import { parseIcon } from "../icons/icon-grammar";
import type { AppStore } from "../store";
import { StoreContext } from "../store-context";
import { PageGlyph, SpaceGlyph } from "./icons";

export { parseIcon } from "../icons/icon-grammar";
export type { ParsedIcon } from "../icons/icon-grammar";

export interface PageIconProps {
  /** The icon string of the page or space (`undefined` for none). */
  icon: string | null | undefined;
  /** Which default glyph shows when there is no icon, or one that cannot be shown. */
  kind?: "page" | "space";
  /** Sizes and places the icon: it lands on the `span` this component renders. */
  className?: string;
}

type Shown = "emoji" | "set" | "image" | "default";

function Frame({ shown, className, children }: { shown: Shown; className: string | undefined; children: ReactNode }) {
  return (
    <span className={className === undefined ? "page-icon" : `page-icon ${className}`} data-icon-kind={shown} aria-hidden="true">
      {children}
    </span>
  );
}

function Fallback({ kind }: { kind: "page" | "space" }) {
  return kind === "space" ? <SpaceGlyph /> : <PageGlyph />;
}

/** An uploaded picture: its `blob:` URL once the bytes are read, the default glyph until then. */
function UploadedIcon({
  store,
  attachmentId,
  kind,
  className,
}: {
  store: AppStore;
  attachmentId: string;
  kind: "page" | "space";
  className: string | undefined;
}) {
  const url = useIconImage(store, attachmentId);
  // A picture the webview cannot decode shows the glyph too.
  const [broken, setBroken] = useState<string | null>(null);
  if (url === null || url === broken) {
    return (
      <Frame shown="default" className={className}>
        <Fallback kind={kind} />
      </Frame>
    );
  }
  return (
    <Frame shown="image" className={className}>
      <img
        className="page-icon-image"
        src={url}
        alt=""
        draggable={false}
        onError={() => {
          setBroken(url);
        }}
      />
    </Frame>
  );
}

/** The icon of a page or space; see the top of this file. */
export function PageIcon({ icon, kind = "page", className }: PageIconProps) {
  const store = useContext(StoreContext);
  const parsed = parseIcon(icon);
  if (parsed?.kind === "emoji") {
    return (
      <Frame shown="emoji" className={className}>
        {parsed.text}
      </Frame>
    );
  }
  if (parsed?.kind === "set") {
    const entry = iconByName(parsed.name);
    if (entry !== undefined) {
      const Glyph = entry.component;
      const colour = parsed.color === undefined ? "" : ` page-icon-color-${parsed.color}`;
      return (
        <Frame shown="set" className={className}>
          <Glyph className={`page-icon-set${colour}`} size="1em" focusable="false" />
        </Frame>
      );
    }
  }
  // A space's icon is never a picture (the core refuses it): one found there shows the glyph.
  if (parsed?.kind === "image" && kind === "page" && store !== null) {
    return <UploadedIcon store={store} attachmentId={parsed.attachmentId} kind={kind} className={className} />;
  }
  return (
    <Frame shown="default" className={className}>
      <Fallback kind={kind} />
    </Frame>
  );
}
