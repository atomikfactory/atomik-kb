// Markdown from a model, drawn as React elements (ADR-0037, `docs/architecture/ai.md` section 8.4): the text is
// parsed by the core client's Markdown parser into portable nodes (the same ones a page is made of) and every node
// becomes an element here. Nothing is ever set as HTML, and a link is not a link: its text is shown (with the
// address as a tooltip), because model output must not be able to take the window anywhere.
//
// Used by the chat panel (with `sources`, so `[n]` citations become chips that open the page) and by the writing
// panel's preview (without them: a citation stays text).

import { citationParts, dateMentionText, parseMarkdown } from "@atomik-kb/core-client";
import type { AiSource, PortableMark, PortableNode } from "@atomik-kb/core-client";
import { Fragment, useMemo } from "react";
import type { ReactNode } from "react";

import { t } from "../i18n";

export interface AiMarkdownProps {
  /** The Markdown to show; it may be unfinished (an answer that is still streaming). */
  markdown: string;
  /**
   * The passages `[n]` citations refer to. A citation of a source in the list is a chip with the page's title;
   * without this prop, or for a number with no source, it stays the text `[n]`.
   */
  sources?: readonly AiSource[];
  /** A citation chip was pressed: open the page and block of the source. */
  onCite?: (source: AiSource) => void;
  className?: string;
}

interface Context {
  sources: readonly AiSource[] | undefined;
  onCite: ((source: AiSource) => void) | undefined;
}

function hasMark(marks: readonly PortableMark[], type: string): boolean {
  return marks.some((mark) => mark.type === type);
}

/** A citation chip: the number and the page's title; pressing it opens the source. */
function Cite({ source, onCite }: { source: AiSource; onCite: ((source: AiSource) => void) | undefined }) {
  const title = source.title === "" ? t("page.untitled") : source.title;
  return (
    <button
      type="button"
      className="ai-cite"
      title={title}
      aria-label={t("aiChat.cite", { n: source.n, title })}
      onClick={() => {
        onCite?.(source);
      }}
    >
      <span className="ai-cite-n">{source.n}</span>
      <span className="ai-cite-title">{title}</span>
    </button>
  );
}

/** The text of a text node with its citations drawn as chips (outside code). */
function textParts(text: string, ctx: Context, plain: boolean): ReactNode[] {
  if (plain || ctx.sources === undefined) {
    return [text];
  }
  const sources = ctx.sources;
  return citationParts(text).map((part, index): ReactNode => {
    if (part.type === "text") {
      return part.text;
    }
    const source = sources.find((candidate) => candidate.n === part.n);
    if (source === undefined) {
      return `[${String(part.n)}]`;
    }
    return <Cite key={index} source={source} onCite={ctx.onCite} />;
  });
}

function inlineNode(node: PortableNode, ctx: Context, key: number): ReactNode {
  switch (node.type) {
    case "text": {
      const marks = node.marks;
      const code = hasMark(marks, "code");
      let element: ReactNode = <>{textParts(node.text ?? "", ctx, code)}</>;
      if (code) {
        element = <code>{node.text ?? ""}</code>;
      }
      if (hasMark(marks, "bold")) {
        element = <strong>{element}</strong>;
      }
      if (hasMark(marks, "italic")) {
        element = <em>{element}</em>;
      }
      if (hasMark(marks, "strike")) {
        element = <s>{element}</s>;
      }
      if (hasMark(marks, "underline")) {
        element = <u>{element}</u>;
      }
      const link = marks.find((mark) => mark.type === "link");
      if (link !== undefined) {
        const href = link.attrs?.href;
        element = (
          <span className="ai-md-link" title={typeof href === "string" ? href : undefined}>
            {element}
          </span>
        );
      }
      return <Fragment key={key}>{element}</Fragment>;
    }
    case "hardBreak":
      return <br key={key} />;
    case "mathInline": {
      const latex = node.attrs?.latex;
      return <code key={key}>{typeof latex === "string" ? latex : ""}</code>;
    }
    case "dateMention":
      return <Fragment key={key}>{dateMentionText(node.attrs?.date, node.attrs?.time) ?? ""}</Fragment>;
    default:
      return null;
  }
}

function inlines(nodes: readonly PortableNode[], ctx: Context): ReactNode[] {
  return nodes.map((node, index) => inlineNode(node, ctx, index));
}

/** The text of a node that holds only text (a code block, a summary). */
function plainText(node: PortableNode): string {
  return (node.text ?? "") + node.content.map(plainText).join("");
}

function blocks(nodes: readonly PortableNode[], ctx: Context): ReactNode[] {
  return nodes.map((node, index) => block(node, ctx, index));
}

function block(node: PortableNode, ctx: Context, key: number): ReactNode {
  switch (node.type) {
    case "paragraph":
      return <p key={key}>{inlines(node.content, ctx)}</p>;
    case "heading": {
      const level = typeof node.attrs?.level === "number" ? Math.min(3, Math.max(1, node.attrs.level)) : 1;
      return (
        <div key={key} className={`ai-md-heading ai-md-h${String(level)}`} role="heading" aria-level={level + 3}>
          {inlines(node.content, ctx)}
        </div>
      );
    }
    case "bulletList":
      return <ul key={key}>{blocks(node.content, ctx)}</ul>;
    case "orderedList": {
      const start = typeof node.attrs?.start === "number" ? node.attrs.start : undefined;
      return (
        <ol key={key} start={start}>
          {blocks(node.content, ctx)}
        </ol>
      );
    }
    case "listItem":
      return <li key={key}>{blocks(node.content, ctx)}</li>;
    case "taskList":
      return (
        <ul key={key} className="ai-md-tasks">
          {blocks(node.content, ctx)}
        </ul>
      );
    case "taskItem": {
      const checked = node.attrs?.checked === true;
      return (
        <li key={key}>
          <input type="checkbox" checked={checked} disabled aria-label={checked ? t("aiChat.task.done") : t("aiChat.task.open")} readOnly />
          <div>{blocks(node.content, ctx)}</div>
        </li>
      );
    }
    case "blockquote":
      return <blockquote key={key}>{blocks(node.content, ctx)}</blockquote>;
    case "callout": {
      const icon = node.attrs?.icon;
      return (
        <blockquote key={key} className="ai-md-callout">
          {typeof icon === "string" && <span aria-hidden="true">{icon} </span>}
          {blocks(node.content, ctx)}
        </blockquote>
      );
    }
    case "codeBlock":
    case "mathBlock":
      return (
        <pre key={key}>
          <code>{plainText(node)}</code>
        </pre>
      );
    case "horizontalRule":
      return <hr key={key} />;
    case "details": {
      const [summary, ...rest] = node.content;
      return (
        <details key={key} open={node.attrs?.open === true}>
          <summary>{summary === undefined ? "" : plainText(summary)}</summary>
          {blocks(rest.flatMap((part) => (part.type === "detailsContent" ? part.content : [part])), ctx)}
        </details>
      );
    }
    case "table":
      return (
        <div key={key} className="ai-md-table">
          <table>
            <tbody>
              {node.content.map((row, rowIndex) => (
                <tr key={rowIndex}>
                  {row.content.map((cell, cellIndex) =>
                    cell.type === "tableHeader" ? (
                      <th key={cellIndex}>{blocks(cell.content, ctx)}</th>
                    ) : (
                      <td key={cellIndex}>{blocks(cell.content, ctx)}</td>
                    ),
                  )}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    case "image": {
      // A model cannot show a picture: its alt text, if it has one, stands for it.
      const alt = node.attrs?.alt;
      return typeof alt === "string" && alt !== "" ? <p key={key}>{alt}</p> : null;
    }
    case "file":
      return null;
    default:
      // A node this renderer does not know: what it holds is still shown.
      return node.content.length > 0 ? <div key={key}>{blocks(node.content, ctx)}</div> : null;
  }
}

/**
 * Renders Markdown from a model into React elements: paragraphs, headings (as labelled text), lists, task lists,
 * quotes, code, rules, tables and the inline marks. Links are drawn as text. `[n]` citations are chips when
 * `sources` has number `n`. Never uses `innerHTML`.
 */
export function AiMarkdown({ markdown, sources, onCite, className }: AiMarkdownProps) {
  const nodes = useMemo(() => parseMarkdown(markdown), [markdown]);
  const ctx: Context = { sources, onCite };
  return <div className={className === undefined ? "ai-md" : `ai-md ${className}`}>{blocks(nodes, ctx)}</div>;
}
