import { MemoryTransport } from "@atomik-kb/core-client";
import type { Transport } from "@atomik-kb/core-client";
import { screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t } from "./i18n";
import { findMainNavigation, recordingTransport, renderApp, renderWith } from "./test/render";
import type { User } from "./test/render";

async function findNameInput(): Promise<HTMLInputElement> {
  return screen.findByLabelText<HTMLInputElement>(t("onboarding.nameLabel"));
}

/** The second step (the recovery kit, covered by onboarding-kit.test.tsx), left for later. */
async function skipKit(user: User): Promise<void> {
  await user.click(await screen.findByRole("button", { name: t("kit.later") }));
}

describe("onboarding", () => {
  it("asks for a workspace name, focused and ready to type", async () => {
    renderApp();
    const input = await findNameInput();
    expect(input).toHaveFocus();
    expect(input).toHaveValue("");
    expect(screen.getByRole("button", { name: t("onboarding.submit") })).toBeEnabled();
  });

  it("does not send anything while the name is blank, and says why", async () => {
    const memory = new MemoryTransport();
    const recorder = recordingTransport(memory);
    const { user } = renderWith(recorder.transport);
    const input = await findNameInput();
    const requestsAfterBoot = recorder.requests.length;

    await user.click(screen.getByRole("button", { name: t("onboarding.submit") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("onboarding.error.empty"));
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(input).toHaveAccessibleDescription(t("onboarding.error.empty"));

    await user.type(input, "   ");
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("alert")).toHaveTextContent(t("onboarding.error.empty"));
    expect(recorder.requests).toHaveLength(requestsAfterBoot);
  });

  it("clears the error as soon as the user edits the name", async () => {
    const { user } = renderApp();
    const input = await findNameInput();
    await user.click(screen.getByRole("button", { name: t("onboarding.submit") }));
    await screen.findByRole("alert");
    await user.type(input, "P");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(input).toHaveAttribute("aria-invalid", "false");
  });

  it("creates the workspace with the trimmed name, then continues with the recovery kit", async () => {
    const { user, memory } = renderApp();
    await user.type(await findNameInput(), "  Personal  ");
    await user.click(screen.getByRole("button", { name: t("onboarding.submit") }));

    expect(await screen.findByRole("heading", { name: t("kit.title"), level: 1 })).toBeInTheDocument();
    const reply = await memory.dispatch({ type: "WorkspaceGet" });
    expect(reply).toEqual({
      type: "Workspace",
      data: { id: expect.any(String) as unknown, name: "Personal", createdAt: expect.anything() as unknown },
    });
    expect(screen.queryByLabelText(t("onboarding.nameLabel"))).not.toBeInTheDocument();
    expect(screen.queryByRole("navigation")).not.toBeInTheDocument();

    await skipKit(user);
    await findMainNavigation();
    expect(screen.getByRole("heading", { name: "Personal", level: 1 })).toBeInTheDocument();
  });

  it("creates the first space with the workspace, named by the default space name", async () => {
    const { user, memory } = renderApp();
    await user.type(await findNameInput(), "Notes{Enter}");
    await skipKit(user);
    await findMainNavigation();

    const spaces = await memory.dispatch({ type: "SpaceList" });
    expect(spaces).toEqual({
      type: "Spaces",
      data: [expect.objectContaining({ name: t("onboarding.defaultSpaceName"), pageCount: 0 }) as unknown],
    });
    // The sidebar shows that space right away, empty, with the way to the first page.
    expect(await screen.findByRole("button", { name: t("tree.newPage") })).toBeInTheDocument();
    expect(screen.getByRole("button", { description: t("spaces.switchTitle") })).toHaveTextContent(
      t("onboarding.defaultSpaceName"),
    );
    expect(screen.getByText(t("home.hint.emptySpace"))).toBeInTheDocument();
    expect(screen.queryByText(t("spaces.firstTitle"))).not.toBeInTheDocument();
  });

  it("submits with the Enter key", async () => {
    const { user } = renderApp();
    await user.type(await findNameInput(), "Notes{Enter}");
    await skipKit(user);
    await findMainNavigation();
    expect(screen.getByRole("heading", { name: "Notes", level: 1 })).toBeInTheDocument();
  });

  it("shows the core's validation message next to the field and stays on the form", async () => {
    const { user } = renderApp();
    const input = await findNameInput();
    // Pasting is how a name this long gets in; typing 201 characters is just slow.
    await user.click(input);
    await user.paste("x".repeat(201));
    await user.click(screen.getByRole("button", { name: t("onboarding.submit") }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Name must be at most 200 characters.");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("button", { name: t("onboarding.submit") })).toBeEnabled();
    expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
  });

  it("shows control-character errors from the core the same way", async () => {
    const { user } = renderApp();
    const input = await findNameInput();
    await user.click(input);
    await user.paste("bad\u0007name");
    await user.click(screen.getByRole("button", { name: t("onboarding.submit") }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Name must not contain control characters.");
  });

  it("explains unexpected core failures without losing what was typed", async () => {
    const { user, memory } = renderApp();
    const input = await findNameInput();
    await user.type(input, "Personal");
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    await user.click(screen.getByRole("button", { name: t("onboarding.submit") }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("onboarding.error.generic", { message: "database error: disk I/O error" }),
    );
    expect(input).toHaveValue("Personal");
    expect(screen.getByRole("button", { name: t("onboarding.submit") })).toBeEnabled();
  });

  it("says so when a workspace already exists (and the UI was not told about it)", async () => {
    const memory = new MemoryTransport();
    // No events reach the UI, so it still shows the form when the workspace appears.
    const deaf: Transport = {
      dispatch: (request) => memory.dispatch(request),
      coreState: () => memory.coreState(),
      docs: memory.docs,
      subscribe: () => () => undefined,
    };
    const { user } = renderWith(deaf);
    await user.type(await findNameInput(), "Second");
    await memory.dispatch({ type: "WorkspaceCreate", data: { name: "First" } });
    await user.click(screen.getByRole("button", { name: t("onboarding.submit") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("onboarding.error.alreadyExists"));
  });

  it("disables the button and says 'Creating...' while the request is in flight", async () => {
    const memory = new MemoryTransport();
    const recorder = recordingTransport(memory);
    const { user } = renderWith(recorder.transport);
    await user.type(await findNameInput(), "Personal");

    const release = recorder.hold();
    await user.click(screen.getByRole("button", { name: t("onboarding.submit") }));
    const busy = await screen.findByRole("button", { name: t("onboarding.submitting") });
    expect(busy).toBeDisabled();
    // A second submit while busy is ignored.
    await user.type(screen.getByLabelText(t("onboarding.nameLabel")), "{Enter}");
    release();

    await skipKit(user);
    await findMainNavigation();
    const creates = recorder.requests.filter((request) => request.type === "WorkspaceCreate");
    expect(creates).toHaveLength(1);
  });

  it("is replaced by the home screen when another window creates the workspace", async () => {
    const { memory } = renderApp();
    await findNameInput();
    await memory.dispatch({ type: "WorkspaceCreate", data: { name: "From elsewhere" } });
    const nav = await findMainNavigation();
    expect(within(nav).getByText("From elsewhere")).toBeInTheDocument();
  });
});
