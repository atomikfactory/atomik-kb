import { MemoryTransport } from "@atomik-kb/core-client";
import type { Event, Transport } from "@atomik-kb/core-client";
import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import { findMainNavigation, openScreen, renderApp, renderWith } from "./test/render";

describe("boot", () => {
  it("shows the loading screen first, then onboarding for a fresh install", async () => {
    renderApp();
    expect(screen.getByRole("status")).toHaveTextContent(t("loading.title"));
    expect(await screen.findByRole("heading", { name: t("onboarding.title") })).toBeInTheDocument();
    expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
  });

  it("goes straight to the home screen when a workspace already exists", async () => {
    renderApp({ workspaceName: "Research" });
    await findMainNavigation();
    expect(screen.getByRole("heading", { name: "Research", level: 1 })).toBeInTheDocument();
    // This workspace has no space yet (Milestone 1 data), so the sidebar offers to create one.
    expect(screen.getByText(t("home.hint.noSpaces"))).toBeInTheDocument();
    expect(screen.getByText(t("spaces.firstTitle"))).toBeInTheDocument();
  });

  it("shows the pages of the first space and a hint to open one", async () => {
    renderApp({ workspaceName: "Research", spaces: [{ name: "Personal", pages: [{ title: "Welcome" }] }] });
    await findMainNavigation();
    expect(screen.getByRole("treeitem", { name: "Welcome" })).toBeInTheDocument();
    expect(screen.getByText(t("home.hint.select"))).toBeInTheDocument();
  });

  it("keeps showing the loading screen while the core is still opening, then continues", async () => {
    const { memory } = renderApp({ coreState: "opening" });
    // Let a few poll rounds pass: still loading, and no request reached the core.
    await new Promise((resolve) => setTimeout(resolve, 250));
    expect(screen.getByRole("status")).toHaveTextContent(t("loading.title"));
    memory.setCoreState("ready");
    expect(await screen.findByRole("heading", { name: t("onboarding.title") })).toBeInTheDocument();
  });

  it("shows the fatal error screen with code, message and a hint when the core failed to open", async () => {
    renderApp({
      coreState: {
        failed: { code: "KeystoreUnavailable", message: "no usable keystore: no secret service is running" },
      },
    });
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByRole("heading", { name: t("fatal.title") })).toBeInTheDocument();
    expect(within(alert).getByText("KeystoreUnavailable")).toBeInTheDocument();
    expect(within(alert).getByText("no usable keystore: no secret service is running")).toBeInTheDocument();
    expect(within(alert).getByText(t("fatal.hint.KeystoreUnavailable"))).toBeInTheDocument();
  });

  it("falls back to the generic hint for codes without a specific one", async () => {
    renderApp({ coreState: { failed: { code: "Internal", message: "something broke" } } });
    expect(await screen.findByText(t("fatal.hint.default"))).toBeInTheDocument();
  });

  it("treats a failing first request as fatal too", async () => {
    const { memory } = renderApp();
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" }, 3);
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText("Storage")).toBeInTheDocument();
    expect(within(alert).getByText("database error: disk I/O error")).toBeInTheDocument();
  });

  it("recovers through 'Try again' once the cause is gone", async () => {
    const { memory, user } = renderApp({
      coreState: { failed: { code: "KeystoreUnavailable", message: "no usable keystore" } },
    });
    await screen.findByRole("alert");
    memory.setCoreState("ready");
    await user.click(screen.getByRole("button", { name: t("fatal.retry") }));
    expect(await screen.findByRole("heading", { name: t("onboarding.title") })).toBeInTheDocument();
  });
});

describe("navigation", () => {
  it("moves between home, settings and diagnostics and marks the current page", async () => {
    const { user } = renderApp({ workspaceName: "Research" });
    const nav = await findMainNavigation();
    const current = () => within(nav).getByRole("button", { current: "page" });
    expect(current()).toHaveTextContent(t("nav.home"));

    await openScreen(user, "nav.settings");
    expect(await screen.findByRole("heading", { name: t("settings.title") })).toBeInTheDocument();
    expect(current()).toHaveTextContent(t("nav.settings"));

    await openScreen(user, "nav.diagnostics");
    expect(await screen.findByRole("heading", { name: t("diagnostics.title") })).toBeInTheDocument();
    expect(current()).toHaveTextContent(t("nav.diagnostics"));

    await openScreen(user, "nav.home");
    expect(screen.getByRole("heading", { name: "Research", level: 1 })).toBeInTheDocument();
  });

  it("shows the workspace name in the sidebar on every screen", async () => {
    const { user } = renderApp({ workspaceName: "Research" });
    const nav = await findMainNavigation();
    await openScreen(user, "nav.settings");
    expect(within(nav).getByText("Research")).toBeInTheDocument();
  });
});

describe("lifecycle", () => {
  it("applies the theme to the document root and removes it when unmounted", async () => {
    const { unmount } = renderApp({ workspaceName: "Research", settings: { "ui.theme": "dark" } });
    await waitFor(() => {
      expect(document.documentElement).toHaveAttribute("data-theme", "dark");
    });
    unmount();
    expect(document.documentElement).not.toHaveAttribute("data-theme");
  });

  it("applies the accent to the document root with the theme, and removes it when unmounted", async () => {
    const { unmount } = renderApp({
      workspaceName: "Research",
      settings: { "ui.theme": "dark", "ui.accent": "green" },
    });
    await waitFor(() => {
      expect(document.documentElement).toHaveAttribute("data-accent", "green");
    });
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
    unmount();
    expect(document.documentElement).not.toHaveAttribute("data-accent");
  });

  it("leaves the root without an accent when none is stored, or when it is the default blue", async () => {
    const first = renderApp({ workspaceName: "Research" });
    await findMainNavigation();
    expect(document.documentElement).toHaveAttribute("data-theme", "system");
    expect(document.documentElement).not.toHaveAttribute("data-accent");
    first.unmount();

    renderApp({ workspaceName: "Research", settings: { "ui.accent": "blue" } });
    await findMainNavigation();
    expect(document.documentElement).not.toHaveAttribute("data-accent");
  });

  it("has the accent on the root as soon as the app is on the page, so no frame shows the default one", async () => {
    // The accent is read with the theme, before the app leaves the loading screen. A mutation observer's callback runs
    // right after the commit that added the navigation, before the browser could paint.
    let accentWhenAppeared: string | null | undefined;
    const observer = new MutationObserver(() => {
      if (accentWhenAppeared === undefined && document.querySelector("nav") !== null) {
        accentWhenAppeared = document.documentElement.getAttribute("data-accent");
      }
    });
    observer.observe(document.body, { childList: true, subtree: true });
    try {
      renderApp({ workspaceName: "Research", settings: { "ui.accent": "orange" } });
      expect(screen.getByRole("status")).toHaveTextContent(t("loading.title"));
      await findMainNavigation();
      await waitFor(() => {
        expect(accentWhenAppeared).toBeDefined();
      });
      expect(accentWhenAppeared).toBe("orange");
    } finally {
      observer.disconnect();
    }
  });

  it("subscribes once for the mounted app and unsubscribes on unmount, also under StrictMode", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research" });
    let active = 0;
    const transport: Transport = {
      dispatch: (request) => memory.dispatch(request),
      coreState: () => memory.coreState(),
      docs: memory.docs,
      subscribe: (onEvent: (event: Event) => void) => {
        active += 1;
        const stop = memory.subscribe(onEvent);
        return () => {
          active -= 1;
          stop();
        };
      },
    };
    const { unmount } = renderWith(transport, { strict: true });
    await findMainNavigation();
    expect(active).toBe(1);
    unmount();
    expect(active).toBe(0);
  });

  it("stops polling a core that is still opening when the app unmounts", async () => {
    const memory = new MemoryTransport({ coreState: "opening" });
    const coreState = vi.spyOn(memory, "coreState");
    const { unmount } = renderWith(memory);
    await waitFor(() => {
      expect(coreState.mock.calls.length).toBeGreaterThan(1);
    });
    unmount();
    const callsAtUnmount = coreState.mock.calls.length;
    await new Promise((resolve) => setTimeout(resolve, 350));
    // At most one poll that was already in flight when the app unmounted.
    expect(coreState.mock.calls.length).toBeLessThanOrEqual(callsAtUnmount + 1);
  });
});
