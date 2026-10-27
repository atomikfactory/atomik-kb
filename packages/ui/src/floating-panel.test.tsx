// FloatingPanel gives focus back without scrolling: an editor that got focus back jumped to its caret,
// and the next popover of a database view opened far from its button (found by the real-app pass).

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { FloatingPanel } from "./components/FloatingPanel";

function Harness() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        Open
      </button>
      {open && (
        <FloatingPanel anchor={{ x: 0, y: 0 }} label="Settings" onClose={() => setOpen(false)}>
          <button type="button">Inside</button>
        </FloatingPanel>
      )}
    </>
  );
}

describe("FloatingPanel", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("gives focus back to its opener on Escape without scrolling", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    const opener = screen.getByRole("button", { name: "Open" });
    await user.click(opener);
    expect(screen.getByRole("button", { name: "Inside" })).toHaveFocus();

    const focus = vi.spyOn(opener, "focus");
    await user.keyboard("{Escape}");

    expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull();
    expect(opener).toHaveFocus();
    expect(focus).toHaveBeenCalledWith({ preventScroll: true });
  });
});
