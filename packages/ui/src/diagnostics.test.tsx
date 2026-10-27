import { API_VERSION, MemoryTransport } from "@atomik-kb/core-client";
import type { Transport } from "@atomik-kb/core-client";
import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t } from "./i18n";
import { openScreen, renderApp, renderWith } from "./test/render";
import type { MessageKey } from "./i18n";

async function openDiagnostics(options: Parameters<typeof renderApp>[0] = {}) {
  const app = renderApp({ workspaceName: "Research", ...options });
  await openScreen(app.user, "nav.diagnostics");
  await screen.findByRole("heading", { name: t("diagnostics.title") });
  return app;
}

/** The text of the `<dd>` that follows the `<dt>` with this label. */
function field(label: MessageKey): string | null {
  const term = screen.getByText(t(label), { selector: "dt" });
  return term.nextElementSibling?.textContent ?? null;
}

describe("diagnostics", () => {
  it("shows everything the core reports in AppStatus", async () => {
    const { memory } = await openDiagnostics({
      status: {
        schemaVersion: 1,
        dbEncrypted: true,
        dbCipher: "SQLCipher 4.14.0 community",
        coreVersion: "0.1.0",
        platform: "windows",
        dataDir: "C:\\Users\\me\\AppData\\Roaming\\com.atomik-kb.desktop",
      },
    });
    const status = await memory.dispatch({ type: "AppStatus" });
    if (status.type !== "AppStatus") {
      throw new Error("unexpected response");
    }

    expect(field("diagnostics.field.workspace")).toBe(t("common.yes"));
    expect(field("diagnostics.field.schemaVersion")).toBe("1");
    expect(field("diagnostics.field.encrypted")).toBe(t("common.yes"));
    expect(field("diagnostics.field.cipher")).toBe("SQLCipher 4.14.0 community");
    expect(field("diagnostics.field.deviceId")).toBe(status.data.deviceId);
    expect(field("diagnostics.field.coreVersion")).toBe("0.1.0");
    expect(field("diagnostics.field.apiVersion")).toBe(String(API_VERSION));
    expect(field("diagnostics.field.platform")).toBe("windows");
    expect(field("diagnostics.field.dataDir")).toBe("C:\\Users\\me\\AppData\\Roaming\\com.atomik-kb.desktop");
  });

  it("reports an unencrypted database as such", async () => {
    await openDiagnostics({ status: { dbEncrypted: false, dbCipher: "none" } });
    expect(field("diagnostics.field.encrypted")).toBe(t("common.no"));
    expect(field("diagnostics.field.cipher")).toBe("none");
  });

  it("does not warn when the core speaks the API version this build expects", async () => {
    await openDiagnostics();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("warns when the core's API version differs from the one this build was made for", async () => {
    await openDiagnostics({ status: { apiVersion: API_VERSION + 1 } });
    expect(screen.getByRole("alert")).toHaveTextContent(
      t("diagnostics.warning.apiVersion", { expected: API_VERSION, actual: API_VERSION + 1 }),
    );
    expect(field("diagnostics.field.apiVersion")).toBe(String(API_VERSION + 1));
  });

  it("warns for an older core too", async () => {
    await openDiagnostics({ status: { apiVersion: 0 } });
    expect(screen.getByRole("alert")).toHaveTextContent(
      t("diagnostics.warning.apiVersion", { expected: API_VERSION, actual: 0 }),
    );
  });

  it("re-reads the status when 'Refresh' is pressed", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research" });
    let schemaVersion = 1;
    const transport: Transport = {
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs: memory.docs,
      dispatch: async (request) => {
        const response = await memory.dispatch(request);
        // Pretend the core migrated its schema between two reads.
        return response.type === "AppStatus"
          ? { type: "AppStatus", data: { ...response.data, schemaVersion } }
          : response;
      },
    };
    const { user } = renderWith(transport);
    await openScreen(user, "nav.diagnostics");
    await screen.findByRole("heading", { name: t("diagnostics.title") });
    expect(field("diagnostics.field.schemaVersion")).toBe("1");

    schemaVersion = 2;
    await user.click(screen.getByRole("button", { name: t("diagnostics.refresh") }));
    await waitFor(() => {
      expect(field("diagnostics.field.schemaVersion")).toBe("2");
    });
  });

  it("updates by itself when the workspace is created (WorkspaceCreated event)", async () => {
    const { memory, user } = renderApp();
    await screen.findByRole("heading", { name: t("onboarding.title") });
    await memory.dispatch({ type: "WorkspaceCreate", data: { name: "Research" } });
    await openScreen(user, "nav.diagnostics");
    await waitFor(() => {
      expect(field("diagnostics.field.workspace")).toBe(t("common.yes"));
    });
  });

  it("refreshes on demand and reports a failed refresh", async () => {
    const { user, memory } = await openDiagnostics();
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    await user.click(screen.getByRole("button", { name: t("diagnostics.refresh") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("diagnostics.refreshError", { message: "database error: disk I/O error" }),
    );
    // The old values stay on screen and a second try works.
    expect(field("diagnostics.field.platform")).toBe("memory");
    await user.click(screen.getByRole("button", { name: t("diagnostics.refresh") }));
    await waitFor(() => {
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    });
    expect(within(screen.getByRole("region", { name: t("diagnostics.title") })).getByRole("button")).toBeEnabled();
  });
});
