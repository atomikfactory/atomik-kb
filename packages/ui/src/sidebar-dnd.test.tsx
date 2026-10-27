import { MemoryTransport } from "@atomik-kb/core-client";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t } from "./i18n";
import { FakeDataTransfer, beginDrag, dragAndDrop } from "./test/dnd";
import { recordingTransport, renderWith } from "./test/render";
import { PERSONAL, PERSONAL_OUTLINE, outline, renderSidebar, row } from "./test/sidebar";

/** The space below the last row, where a page dropped goes to the end of the root. */
function treeEnd(): HTMLElement {
  const end = screen.getByRole("button", { name: t("tree.newPage") }).parentElement;
  if (end === null) {
    throw new Error("the end of the tree is missing");
  }
  return end;
}

describe("drag and drop: where a page lands", () => {
  it("puts a page before a row when it is dropped on its upper part", async () => {
    await renderSidebar();
    expect(dragAndDrop(row("Journal"), row("Projects"), "before")).toBe(true);
    await waitFor(() => {
      expect(outline()).toEqual(["Journal", "Projects", "  Roadmap", "    Q1", "  Ideas", "Reading list"]);
    });
  });

  it("puts a page after a row when it is dropped on its lower part, taking its subtree along", async () => {
    await renderSidebar();
    expect(dragAndDrop(row("Projects"), row("Reading list"), "after")).toBe(true);
    await waitFor(() => {
      expect(outline()).toEqual(["Journal", "Reading list", "Projects", "  Roadmap", "    Q1", "  Ideas"]);
    });
  });

  it("makes a page the last child of the row it is dropped on the middle of, and expands it", async () => {
    await renderSidebar();
    expect(dragAndDrop(row("Journal"), row("Ideas"), "inside")).toBe(true);
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas", "    Journal", "Reading list"]);
    });
    expect(row("Ideas")).toHaveAttribute("aria-expanded", "true");
  });

  it("expands a collapsed row that a page is dropped into", async () => {
    const { user } = await renderSidebar();
    await user.click(within(row("Projects")).getByRole("button", { name: t("tree.collapse") }));
    expect(outline()).toEqual(["Projects", "Journal", "Reading list"]);
    dragAndDrop(row("Journal"), row("Projects"), "inside");
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas", "  Journal", "Reading list"]);
    });
  });

  it("drops after a row with children as its sibling, after the whole subtree", async () => {
    await renderSidebar();
    dragAndDrop(row("Reading list"), row("Projects"), "after");
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas", "Reading list", "Journal"]);
    });
  });

  it("drops before a nested row as a child of the same parent", async () => {
    await renderSidebar();
    dragAndDrop(row("Journal"), row("Ideas"), "before");
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Journal", "  Ideas", "Reading list"]);
    });
  });

  it("counts positions without the dragged page when it moves within its parent", async () => {
    await renderSidebar();
    dragAndDrop(row("Roadmap"), row("Ideas"), "after");
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Ideas", "  Roadmap", "    Q1", "Journal", "Reading list"]);
    });
    dragAndDrop(row("Ideas"), row("Roadmap"), "after");
    await waitFor(() => {
      expect(outline()).toEqual(PERSONAL_OUTLINE);
    });
  });

  it("moves a page to the end of the root when it is dropped below the last row", async () => {
    await renderSidebar();
    const drag = beginDrag(row("Ideas"));
    const dataTransfer = drag.dataTransfer;
    expect(fireEvent.dragOver(treeEnd(), { dataTransfer })).toBe(false);
    expect(treeEnd()).toHaveClass("is-drop-target");
    fireEvent.drop(treeEnd(), { dataTransfer });
    drag.end();
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "Journal", "Reading list", "Ideas"]);
    });
    expect(treeEnd()).not.toHaveClass("is-drop-target");
  });

  it("does not accept a page below the last row when it is already the last root page", async () => {
    await renderSidebar();
    const drag = beginDrag(row("Reading list"));
    expect(fireEvent.dragOver(treeEnd(), { dataTransfer: drag.dataTransfer })).toBe(true);
    expect(treeEnd()).not.toHaveClass("is-drop-target");
    drag.end();
  });
});

describe("drag and drop: what is not allowed or does nothing", () => {
  it("refuses a drop on the page itself or inside its own subtree", async () => {
    await renderSidebar();
    for (const zone of ["before", "inside", "after"] as const) {
      expect(dragAndDrop(row("Projects"), row("Projects"), zone), `onto itself, ${zone}`).toBe(false);
      expect(dragAndDrop(row("Projects"), row("Roadmap"), zone), `onto a child, ${zone}`).toBe(false);
      expect(dragAndDrop(row("Projects"), row("Q1"), zone), `onto a grandchild, ${zone}`).toBe(false);
    }
    expect(outline()).toEqual(PERSONAL_OUTLINE);
  });

  it("does not highlight a refused target", async () => {
    await renderSidebar();
    const drag = beginDrag(row("Projects"));
    expect(drag.over(row("Roadmap"), "inside")).toBe(false);
    expect(row("Roadmap")).not.toHaveClass("drop-inside");
    drag.end();
  });

  it("does not ask the core to move a page to where it already is", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [PERSONAL] });
    const recorder = recordingTransport(memory);
    renderWith(recorder.transport);
    await screen.findByRole("tree");
    // Journal sits right after Projects and right before Reading list.
    expect(dragAndDrop(row("Journal"), row("Reading list"), "before")).toBe(true);
    expect(dragAndDrop(row("Journal"), row("Projects"), "after")).toBe(true);
    expect(dragAndDrop(row("Reading list"), row("Journal"), "after")).toBe(true);
    expect(recorder.requests.filter((request) => request.type === "PageMove")).toEqual([]);
    expect(outline()).toEqual(PERSONAL_OUTLINE);
  });

  it("ignores drags that did not start on a page of the tree", async () => {
    await renderSidebar();
    const foreign = new FakeDataTransfer();
    foreign.setData("text/plain", "from somewhere else");
    // fireEvent returns true when nothing called preventDefault, which is how a refusal looks.
    expect(fireEvent.dragOver(row("Journal"), { dataTransfer: foreign })).toBe(true);
    fireEvent.drop(row("Journal"), { dataTransfer: foreign });
    expect(row("Journal")).not.toHaveClass("drop-inside");
    expect(outline()).toEqual(PERSONAL_OUTLINE);
  });

  it("shows the zone while dragging over a row, and the dragged row dimmed", async () => {
    await renderSidebar();
    const drag = beginDrag(row("Journal"));
    expect(row("Journal")).toHaveClass("is-dragging");
    const target = row("Reading list");
    drag.over(target, "before");
    expect(target).toHaveClass("drop-before");
    drag.over(target, "inside");
    expect(target).toHaveClass("drop-inside");
    expect(target).not.toHaveClass("drop-before");
    drag.over(target, "after");
    expect(target).toHaveClass("drop-after");
    drag.leave(target);
    expect(target).not.toHaveClass("drop-after");
    drag.over(target, "inside");
    drag.end();
    expect(target).not.toHaveClass("drop-inside");
    expect(row("Journal")).not.toHaveClass("is-dragging");
  });

  it("clears the highlight after a drop", async () => {
    await renderSidebar();
    dragAndDrop(row("Journal"), row("Ideas"), "inside");
    await waitFor(() => {
      expect(outline()).toContain("    Journal");
    });
    for (const item of screen.getAllByRole("treeitem")) {
      expect(item.className).toBe("tree-row");
    }
  });

  it("explains a refused move and leaves the tree as it was", async () => {
    const { memory } = await renderSidebar();
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    dragAndDrop(row("Journal"), row("Projects"), "before");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("tree.error.move", { message: "database error: disk I/O error" }),
    );
    expect(outline()).toEqual(PERSONAL_OUTLINE);
  });

  it("does not drag a row while its title is being edited", async () => {
    const { user } = await renderSidebar();
    expect(row("Journal")).toHaveAttribute("draggable", "true");
    await user.dblClick(row("Journal"));
    await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    expect(row("Journal")).toHaveAttribute("draggable", "false");
  });
});
