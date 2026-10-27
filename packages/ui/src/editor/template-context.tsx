// Which page the editor shows, for the node views inside it (ADR-0056): the date and person mentions of a page in a template can be
// relative ("Today", "Now", "Me" filled in when a page is made from the template), and only there do they offer it. The page editor
// provides the id of its page; a node view asks whether that page is a template or inside one.

import { createContext, useContext } from "react";

import { useAppStore } from "../store-context";
import { templateRootOf } from "../template-tree";

/** The id of the page the surrounding editor shows; `null` outside a page editor (a schema-only editor, a test). */
export const EditorPageContext = createContext<string | null>(null);

/** Whether the page of the surrounding editor is a template or inside one; follows the flag while the page is open. */
export function useInTemplatePage(): boolean {
  const pageId = useContext(EditorPageContext);
  return useAppStore((state) => pageId !== null && templateRootOf(state.tree, pageId) !== null);
}
