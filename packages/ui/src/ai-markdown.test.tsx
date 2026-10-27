// Markdown from a model drawn as React elements (components/AiMarkdown.tsx, ADR-0037): the blocks and marks, links
// that are not links, citations as chips, and the promise that nothing a model writes becomes HTML.

import type { AiSource } from "@atomik-kb/core-client";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { AiMarkdown } from "./components/AiMarkdown";
import { t } from "./i18n";

function show(markdown: string, props: { sources?: readonly AiSource[]; onCite?: (source: AiSource) => void } = {}) {
  const view = render(<AiMarkdown markdown={markdown} {...props} />);
  return { ...view, root: view.container.firstElementChild as HTMLElement };
}

const SOURCES: AiSource[] = [
  { n: 1, pageId: "p1", spaceId: "s1", blockId: "b1", title: "Garden plans", text: "Tomatoes", score: 1 },
  { n: 2, pageId: "p2", spaceId: "s1", title: "", text: "Soup", score: 0.5 },
];

describe("blocks", () => {
  it("draws paragraphs, and an empty text as an empty container", () => {
    const { root } = show("One.\n\nTwo with a  \nbreak.");
    expect(root).toHaveClass("ai-md");
    expect(root.querySelectorAll("p")).toHaveLength(2);
    expect(root.querySelector("br")).toBeInTheDocument();
    expect(show("").root).toBeEmptyDOMElement();
  });

  it("draws headings as labelled text, with a level that follows the Markdown level (to 3)", () => {
    const { root } = show("# One\n\n## Two\n\n### Three\n\n#### Four");
    const headings = within(root).getAllByRole("heading");
    expect(headings.map((heading) => heading.textContent)).toEqual(["One", "Two", "Three", "Four"]);
    expect(headings.map((heading) => heading.getAttribute("aria-level"))).toEqual(["4", "5", "6", "6"]);
    expect(headings[0]).toHaveClass("ai-md-h1");
  });

  it("draws bullet, numbered and task lists", () => {
    const { root } = show("- a\n- b\n  - nested\n\n3. three\n4. four\n\n- [ ] open\n- [x] done");
    expect(root.querySelectorAll("ul > li").length).toBeGreaterThanOrEqual(5);
    expect(root.querySelector("ol")).toHaveAttribute("start", "3");
    expect(within(root).getByText("nested").closest("ul")?.parentElement?.closest("ul")).not.toBeNull();
    const tasks = within(root).getAllByRole("checkbox");
    expect(tasks).toHaveLength(2);
    expect(tasks[0]).not.toBeChecked();
    expect(tasks[1]).toBeChecked();
    expect(tasks.every((box) => (box as HTMLInputElement).disabled)).toBe(true);
    expect(tasks[1]).toHaveAccessibleName(t("aiChat.task.done"));
    expect(tasks[0]).toHaveAccessibleName(t("aiChat.task.open"));
  });

  it("draws code as text, quotes, rules and tables", () => {
    const { root } = show("```ts\nlet x = <b>1</b>;\n```\n\n> quoted\n\n---\n\n| a | b |\n| --- | --- |\n| 1 | 2 |");
    expect(root.querySelector("pre code")?.textContent).toBe("let x = <b>1</b>;");
    expect(root.querySelector("pre code b")).toBeNull();
    expect(root.querySelector("blockquote")).toHaveTextContent("quoted");
    expect(root.querySelector("hr")).toBeInTheDocument();
    expect(within(root).getAllByRole("columnheader").map((cell) => cell.textContent)).toEqual(["a", "b"]);
    expect(within(root).getAllByRole("cell").map((cell) => cell.textContent)).toEqual(["1", "2"]);
  });

  it("draws toggles and equations as text", () => {
    const { root } = show("<details>\n<summary>More</summary>\n\nHidden text\n\n</details>\n\n$$\nE = mc^2\n$$\n\nand $x^2$ inline");
    expect(root.querySelector("summary")).toHaveTextContent("More");
    expect(root).toHaveTextContent("Hidden text");
    expect(root.querySelectorAll("pre code")[0]?.textContent).toBe("E = mc^2");
    expect(within(root).getByText("x^2").tagName).toBe("CODE");
  });
});

describe("marks", () => {
  it("draws bold, italic, strikethrough, underline and code", () => {
    const { root } = show("**bold** *italic* ~~gone~~ <u>under</u> `code` ***both***");
    expect(root.querySelector("strong")).toHaveTextContent("bold");
    expect(root.querySelector("em")).toHaveTextContent("italic");
    expect(root.querySelector("s")).toHaveTextContent("gone");
    expect(root.querySelector("u")).toHaveTextContent("under");
    expect(root.querySelector("code")).toHaveTextContent("code");
    expect(root.querySelector("strong em, em strong")).toHaveTextContent("both");
  });

  it("draws a link as its text with the address as a tooltip, never as a link", () => {
    const { root } = show("see [the docs](https://example.com/a?b=c) and [bad](javascript:alert(1))");
    expect(root.querySelector("a")).toBeNull();
    const link = within(root).getByText("the docs");
    expect(link.closest(".ai-md-link")).toHaveAttribute("title", "https://example.com/a?b=c");
    expect(within(root).getByText("bad").closest(".ai-md-link")).toHaveAttribute("title", "javascript:alert(1)");
  });

  it("does not take the address of a link for a citation", () => {
    const { root } = show("[1](https://example.com)", { sources: SOURCES });
    expect(within(root).queryByRole("button")).toBeNull();
  });
});

describe("what a model writes never becomes HTML", () => {
  it("shows markup as text", () => {
    const { root } = show('<img src=x onerror="alert(1)"> <script>alert(1)</script> <b>bold?</b>\n\n<iframe src="https://evil"></iframe>');
    expect(root.querySelector("img, script, iframe, b")).toBeNull();
    expect(root.textContent).toContain('<img src=x onerror="alert(1)">');
    expect(root.textContent).toContain("<script>alert(1)</script>");
    expect(root.textContent).toContain("<b>bold?</b>");
  });

  it("shows entities and angle brackets as typed", () => {
    const { root } = show("1 < 2 && 3 > 2 &amp; &lt;tag&gt;");
    expect(root.textContent).toBe("1 < 2 && 3 > 2 &amp; &lt;tag&gt;");
  });

  it("does not use innerHTML: the nodes are built by React", () => {
    const set = vi.spyOn(Element.prototype, "innerHTML", "set");
    show("# Title\n\n**bold** and [link](https://example.com) and [1]", { sources: SOURCES });
    expect(set).not.toHaveBeenCalled();
    set.mockRestore();
  });

  it("leaves out pictures and files: the alt text stands in for a picture", () => {
    const id = "00000000-0000-4000-8000-000000000001";
    const { root } = show(`![A cat](attachment://${id})\n\n[notes.txt](attachment://${id})\n\ntext`);
    expect(root.querySelector("img")).toBeNull();
    expect(root).toHaveTextContent("A cat");
    expect(root).toHaveTextContent("text");
  });
});

describe("citations", () => {
  it("are chips with the source's number and its page's title, when the source is known", () => {
    const { root } = show("The beans go in late [1] and the soup is simple [2].", { sources: SOURCES });
    const chips = within(root).getAllByRole("button");
    expect(chips).toHaveLength(2);
    expect(chips[0]).toHaveAccessibleName(t("aiChat.cite", { n: 1, title: "Garden plans" }));
    expect(chips[0]).toHaveTextContent("1Garden plans");
    // A source without a title is "Untitled".
    expect(chips[1]).toHaveAccessibleName(t("aiChat.cite", { n: 2, title: t("page.untitled") }));
    expect(root.textContent).toBe("The beans go in late 1Garden plans and the soup is simple 2Untitled.");
  });

  it("open the source when pressed", async () => {
    const onCite = vi.fn();
    const { root } = show("See [1].", { sources: SOURCES, onCite });
    await userEvent.setup().click(within(root).getByRole("button"));
    expect(onCite).toHaveBeenCalledTimes(1);
    expect(onCite).toHaveBeenCalledWith(SOURCES[0]);
  });

  it("stay text for a number with no source, and without sources at all", () => {
    expect(within(show("Only [3] here.", { sources: SOURCES }).root).queryByRole("button")).toBeNull();
    expect(show("Only [3] here.", { sources: SOURCES }).root.textContent).toBe("Only [3] here.");
    expect(within(show("Plain [1].").root).queryByRole("button")).toBeNull();
    expect(show("Plain [1].").root.textContent).toBe("Plain [1].");
  });

  it("read lists of numbers, and are not made inside code", () => {
    const { root } = show("Both [1, 2] and `code [1]`", { sources: SOURCES });
    expect(within(root).getAllByRole("button")).toHaveLength(2);
    expect(root.querySelector("code")?.textContent).toBe("code [1]");
    expect(root.querySelector("code button")).toBeNull();
  });

  it("follow an answer that is still arriving", () => {
    const view = render(<AiMarkdown markdown="Half a sent" sources={SOURCES} />);
    expect(view.container).toHaveTextContent("Half a sent");
    view.rerender(<AiMarkdown markdown="Half a sentence [1" sources={SOURCES} />);
    expect(screen.queryByRole("button")).toBeNull();
    view.rerender(<AiMarkdown markdown="Half a sentence [1]" sources={SOURCES} />);
    expect(screen.getByRole("button")).toBeInTheDocument();
  });

  it("take a class from the caller", () => {
    const { root } = show("x");
    expect(root).toHaveClass("ai-md");
    const view = render(<AiMarkdown markdown="x" className="mine" />);
    expect(view.container.firstElementChild).toHaveClass("ai-md", "mine");
  });
});
