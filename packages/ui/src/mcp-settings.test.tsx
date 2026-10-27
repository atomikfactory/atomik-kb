// Settings -> AI -> MCP server (ADR-0037, screens/McpSettings.tsx): the switch, port and status, the clients (a token
// shown once, the Claude Desktop configuration and the Claude Code command), the grid of Read and Write per space
// (write implies read; a declined native dialog changes nothing), deleting a client and the audit log.

import { MemoryTransport } from "@atomik-kb/core-client";
import type { Transport } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import { claudeCodeCommand, claudeDesktopConfig } from "./screens/McpSettings";
import { AI_SPACES, MCP_SETTING, aiOf } from "./test/ai";
import { openScreen, renderApp, renderWith } from "./test/render";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const BRIDGE = "C:\\Program Files\\atomik-kb\\kb-mcp-bridge.exe";

type App = Awaited<ReturnType<typeof openSettings>>;

async function openSettings(options: Parameters<typeof renderApp>[0] = {}) {
  const app = renderApp({ workspaceName: "Research", spaces: AI_SPACES, ai: { mode: "fake", bridgePath: BRIDGE }, ...options });
  await goToSettings(app);
  return app;
}

async function goToSettings(app: Pick<ReturnType<typeof renderApp>, "user">) {
  await openScreen(app.user, "nav.settings");
  await screen.findByRole("heading", { name: t("settings.title") });
  await screen.findByRole("heading", { name: t("mcp.title") });
}

const section = () => screen.getByRole("region", { name: t("mcp.title") });

/** Adds a client through the form and returns the block that shows its token. */
async function addClient(app: App, name: string): Promise<HTMLElement> {
  await app.user.type(within(section()).getByLabelText(t("mcp.add.label")), name);
  await app.user.click(within(section()).getByRole("button", { name: t("mcp.add.button") }));
  return within(section()).findByRole("region", { name: t("mcp.new.title", { name }) });
}

describe("the section", () => {
  it("is not there without MCP, and AI settings stay when only MCP is missing", async () => {
    const app = renderApp({ workspaceName: "Research" });
    await openScreen(app.user, "nav.settings");
    await screen.findByRole("heading", { name: t("settings.title") });
    expect(screen.queryByRole("heading", { name: t("mcp.title") })).not.toBeInTheDocument();
  });

  it("is off by default, on the default port, with nothing to show yet", async () => {
    await openSettings();
    expect(within(section()).getByRole("switch", { name: t("mcp.enabled") })).not.toBeChecked();
    expect(within(section()).getByLabelText(t("mcp.port"))).toHaveValue(47811);
    expect(within(section()).getByText(t("mcp.status.off"))).toBeInTheDocument();
    expect(await within(section()).findByText(t("mcp.clients.none"))).toBeInTheDocument();
    expect(within(section()).getByText(t("mcp.audit.none"))).toBeInTheDocument();
  });

  it("stores the switch and says whether the server listens", async () => {
    const app = await openSettings();
    aiOf(app.memory).setMcpStatus({ running: true });
    await app.user.click(within(section()).getByRole("switch", { name: t("mcp.enabled") }));
    expect(await within(section()).findByText(t("mcp.status.running", { address: "127.0.0.1:47811" }))).toBeInTheDocument();
    const stored = await app.memory.dispatch({ type: "SettingsGet", data: { key: MCP_SETTING } });
    expect(stored).toMatchObject({ data: { enabled: true, port: 47811 } });
  });

  it("says why the server is not running", async () => {
    const app = await openSettings();
    aiOf(app.memory).setMcpStatus({ running: false, error: "port 47811 is in use" });
    await app.user.click(within(section()).getByRole("switch", { name: t("mcp.enabled") }));
    expect(await within(section()).findByText(t("mcp.status.error", { message: "port 47811 is in use" }))).toBeInTheDocument();
    aiOf(app.memory).setMcpStatus({ error: undefined });
    await app.user.click(within(section()).getByRole("switch", { name: t("mcp.enabled") }));
    expect(await within(section()).findByText(t("mcp.status.off"))).toBeInTheDocument();
    await app.user.click(within(section()).getByRole("switch", { name: t("mcp.enabled") }));
    expect(await within(section()).findByText(t("mcp.status.stopped"))).toBeInTheDocument();
  });

  it("stores a new port when the field is left or Enter is pressed, and refuses a bad one", async () => {
    const app = await openSettings();
    const port = within(section()).getByLabelText(t("mcp.port"));
    await app.user.clear(port);
    await app.user.type(port, "50123{Enter}");
    await waitFor(async () => {
      const stored = await app.memory.dispatch({ type: "SettingsGet", data: { key: MCP_SETTING } });
      expect(stored).toMatchObject({ data: { port: 50123 } });
    });
    await app.user.clear(port);
    await app.user.type(port, "80");
    await app.user.tab();
    expect(await within(section()).findByRole("alert")).toHaveTextContent(t("mcp.port.error"));
    expect(port).toHaveValue(50123);
    await app.user.clear(port);
    await app.user.type(port, "70000{Enter}");
    expect(await within(section()).findByRole("alert")).toHaveTextContent(t("mcp.port.error"));
  });
});

describe("adding a client", () => {
  it("shows the token once, with the Claude Desktop configuration and the Claude Code command", async () => {
    const app = await openSettings({ settings: { [MCP_SETTING]: { enabled: true, port: 50000 } } });
    const block = await addClient(app, "Claude Desktop");
    const token = within(block).getByText(/^kbm_[A-Za-z0-9_-]{43}$/).textContent ?? "";
    expect(token).toMatch(/^kbm_/);
    // The configuration is valid JSON that names the bridge, the port and the token.
    const config = JSON.parse(within(block).getByLabelText(t("mcp.new.desktop")).textContent ?? "") as unknown;
    expect(config).toEqual({
      mcpServers: { "atomik-kb": { command: BRIDGE, args: ["--port", "50000"], env: { ATOMIK_KB_MCP_TOKEN: token } } },
    });
    expect(within(block).getByLabelText(t("mcp.new.code")).textContent).toBe(
      `claude mcp add --transport http atomik-kb http://127.0.0.1:50000/mcp --header "Authorization: Bearer ${token}"`,
    );
    expect(within(block).getByText(t("mcp.new.once"))).toBeInTheDocument();
    expect(within(block).queryByText(t("mcp.new.noBridge"))).not.toBeInTheDocument();
    // The client is listed, without access.
    const list = within(section()).getByRole("list");
    expect(within(list).getByText("Claude Desktop")).toBeInTheDocument();
    expect(within(list).getByText(t("mcp.access.none"))).toBeInTheDocument();
  });

  it("forgets the token when done, and never lists it", async () => {
    const app = await openSettings();
    const block = await addClient(app, "Scripts");
    const token = within(block).getByText(/^kbm_/).textContent ?? "";
    await app.user.click(within(block).getByRole("button", { name: t("mcp.new.done") }));
    expect(screen.queryByText(token)).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain(token);
    expect(within(within(section()).getByRole("list")).getByText("Scripts")).toBeInTheDocument();
  });

  it("copies the token, the configuration and the command, and says when copying fails", async () => {
    const app = await openSettings();
    const block = await addClient(app, "Agent");
    const token = within(block).getByText(/^kbm_/).textContent ?? "";
    await app.user.click(within(block).getByRole("button", { name: t("mcp.new.copyToken") }));
    expect(await navigator.clipboard.readText()).toBe(token);
    expect(await within(block).findByText(t("mcp.copied"))).toBeInTheDocument();
    await app.user.click(within(block).getByRole("button", { name: t("mcp.new.copyConfig") }));
    expect(JSON.parse(await navigator.clipboard.readText())).toHaveProperty("mcpServers.atomik-kb.command", BRIDGE);
    await app.user.click(within(block).getByRole("button", { name: t("mcp.new.copyCommand") }));
    expect(await navigator.clipboard.readText()).toContain("claude mcp add --transport http atomik-kb");
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(new Error("denied"));
    await app.user.click(within(block).getByRole("button", { name: t("mcp.new.copyToken") }));
    expect(await within(block).findByText(t("mcp.copyFailed"))).toBeInTheDocument();
  });

  it("says the bridge was not found, and writes a placeholder, when the shell reports none", async () => {
    const app = await openSettings({ ai: { mode: "fake" } });
    const block = await addClient(app, "Agent");
    expect(within(block).getByText(t("mcp.new.noBridge"))).toBeInTheDocument();
    expect(within(block).getByLabelText(t("mcp.new.desktop")).textContent).toContain("<path to kb-mcp-bridge>");
  });

  it("asks for a name, and shows the core's refusal of a bad one", async () => {
    const app = await openSettings();
    await app.user.click(within(section()).getByRole("button", { name: t("mcp.add.button") }));
    expect(await within(section()).findByRole("alert")).toHaveTextContent(t("mcp.add.empty"));
    await app.user.type(within(section()).getByLabelText(t("mcp.add.label")), "a\u0007b");
    await app.user.click(within(section()).getByRole("button", { name: t("mcp.add.button") }));
    expect(await within(section()).findByRole("alert")).toHaveTextContent(/^Name must not contain control characters/);
  });

  it("builds the configuration the contract shows", () => {
    expect(JSON.parse(claudeDesktopConfig("/opt/kb-mcp-bridge", 47811, "kbm_abc"))).toEqual({
      mcpServers: { "atomik-kb": { command: "/opt/kb-mcp-bridge", args: ["--port", "47811"], env: { ATOMIK_KB_MCP_TOKEN: "kbm_abc" } } },
    });
    expect(claudeCodeCommand("http://127.0.0.1:47811/mcp", "kbm_abc")).toBe(
      'claude mcp add --transport http atomik-kb http://127.0.0.1:47811/mcp --header "Authorization: Bearer kbm_abc"',
    );
  });
});

describe("permissions", () => {
  async function withClient() {
    const app = await openSettings();
    const block = await addClient(app, "Claude Desktop");
    await app.user.click(within(block).getByRole("button", { name: t("mcp.new.done") }));
    await app.user.click(within(section()).getByRole("button", { name: t("mcp.permissions", { name: "Claude Desktop" }) }));
    const grid = within(section()).getByRole("form", { name: t("mcp.grants.title", { name: "Claude Desktop" }) });
    return { app, grid };
  }

  const read = (grid: HTMLElement, space: string) => within(grid).getByRole("checkbox", { name: t("mcp.grants.readIn", { space }) });
  const write = (grid: HTMLElement, space: string) => within(grid).getByRole("checkbox", { name: t("mcp.grants.writeIn", { space }) });

  it("lists every space with Read and Write, nothing allowed at first", async () => {
    const { grid } = await withClient();
    for (const space of ["Personal", "Work"]) {
      expect(read(grid, space)).not.toBeChecked();
      expect(write(grid, space)).not.toBeChecked();
    }
  });

  it("makes writing imply reading, and not reading imply not writing", async () => {
    const { app, grid } = await withClient();
    await app.user.click(write(grid, "Personal"));
    expect(read(grid, "Personal")).toBeChecked();
    expect(write(grid, "Personal")).toBeChecked();
    await app.user.click(read(grid, "Personal"));
    expect(read(grid, "Personal")).not.toBeChecked();
    expect(write(grid, "Personal")).not.toBeChecked();
    await app.user.click(read(grid, "Work"));
    await app.user.click(write(grid, "Work"));
    await app.user.click(write(grid, "Work"));
    expect(read(grid, "Work")).toBeChecked();
    expect(write(grid, "Work")).not.toBeChecked();
  });

  it("saves the grants, lists what the client may do, and the core holds them", async () => {
    const { app, grid } = await withClient();
    await app.user.click(write(grid, "Personal"));
    await app.user.click(read(grid, "Work"));
    await app.user.click(within(grid).getByRole("button", { name: t("mcp.grants.save") }));
    expect(await within(grid).findByText(t("mcp.grants.saved"))).toBeInTheDocument();
    const item = within(section()).getByRole("listitem");
    // The list is read again after the save; the editor is not rebuilt, so its notice stays and its boxes keep the grants.
    expect(await within(item).findByText(`${t("mcp.access.read", { spaces: "Work" })} ${t("mcp.access.write", { spaces: "Personal" })}`)).toBeInTheDocument();
    expect(within(section()).getByRole("form", { name: t("mcp.grants.title", { name: "Claude Desktop" }) })).toBe(grid);
    expect(within(grid).getByText(t("mcp.grants.saved"))).toBeInTheDocument();
    expect(read(grid, "Personal")).toBeChecked();
    expect(write(grid, "Personal")).toBeChecked();
    expect(read(grid, "Work")).toBeChecked();
    expect(write(grid, "Work")).not.toBeChecked();
    const listed = await app.memory.dispatch({ type: "McpClientList" });
    expect(listed.type === "McpClients" ? listed.data[0]?.grants : []).toEqual([
      expect.objectContaining({ read: true, write: true }),
      expect.objectContaining({ read: true, write: false }),
    ]);
  });

  it("closes the grid", async () => {
    const { app, grid } = await withClient();
    await app.user.click(within(grid).getByRole("button", { name: t("mcp.grants.close") }));
    expect(screen.queryByRole("form", { name: t("mcp.grants.title", { name: "Claude Desktop" }) })).not.toBeInTheDocument();
  });

  it("changes nothing when the native confirmation is declined, and says so", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: AI_SPACES, ai: { mode: "fake" } });
    const declining: Transport = {
      dispatch: (request) => memory.dispatch(request),
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs: memory.docs,
      ...(memory.ai === undefined ? {} : { ai: memory.ai }),
      mcp: {
        status: () => memory.mcp?.status() ?? Promise.reject(new Error("no mcp")),
        grantsSet: () =>
          Promise.reject(Object.assign(new Error("declined"), { code: "InvalidInput", details: { field: "grants", reason: "declined" } })),
      },
    };
    const app = renderWith(declining);
    await goToSettings(app);
    await app.user.type(within(section()).getByLabelText(t("mcp.add.label")), "Agent");
    await app.user.click(within(section()).getByRole("button", { name: t("mcp.add.button") }));
    await app.user.click(await within(section()).findByRole("button", { name: t("mcp.new.done") }));
    await app.user.click(within(section()).getByRole("button", { name: t("mcp.permissions", { name: "Agent" }) }));
    const grid = within(section()).getByRole("form", { name: t("mcp.grants.title", { name: "Agent" }) });
    await app.user.click(within(grid).getByRole("checkbox", { name: t("mcp.grants.readIn", { space: "Personal" }) }));
    await app.user.click(within(grid).getByRole("button", { name: t("mcp.grants.save") }));
    expect(await within(grid).findByRole("alert")).toHaveTextContent(t("mcp.grants.declined"));
    const listed = await memory.dispatch({ type: "McpClientList" });
    expect(listed.type === "McpClients" ? listed.data[0]?.grants : null).toEqual([]);
  });
});

describe("deleting a client", () => {
  it("asks first, then deletes it and its key stops working", async () => {
    const app = await openSettings();
    const block = await addClient(app, "Old tool");
    await app.user.click(within(block).getByRole("button", { name: t("mcp.new.done") }));
    await app.user.click(within(section()).getByRole("button", { name: t("mcp.delete", { name: "Old tool" }) }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent(t("mcp.delete.title", { name: "Old tool" }));
    // Cancelling keeps it.
    await app.user.click(within(dialog).getByRole("button", { name: t("dialog.cancel") }));
    expect(within(within(section()).getByRole("list")).getByText("Old tool")).toBeInTheDocument();
    await app.user.click(within(section()).getByRole("button", { name: t("mcp.delete", { name: "Old tool" }) }));
    await app.user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: t("mcp.delete.confirm") }));
    await waitFor(() => {
      expect(within(section()).queryByRole("list")).not.toBeInTheDocument();
    });
    const listed = await app.memory.dispatch({ type: "McpClientList" });
    expect(listed).toMatchObject({ data: [] });
  });
});

describe("the audit log", () => {
  async function withCalls() {
    const app = await openSettings();
    const a = await addClient(app, "Alpha");
    await app.user.click(within(a).getByRole("button", { name: t("mcp.new.done") }));
    const b = await addClient(app, "Beta");
    await app.user.click(within(b).getByRole("button", { name: t("mcp.new.done") }));
    const clients = await app.memory.dispatch({ type: "McpClientList" });
    const [alpha, beta] = clients.type === "McpClients" ? clients.data : [];
    const personal = (await app.memory.dispatch({ type: "SpaceList" }));
    const space = personal.type === "Spaces" ? personal.data[0] : undefined;
    app.memory.recordMcpCall({ clientId: alpha?.id ?? "", tool: "search", outcome: "ok", summary: "tomato", spaceId: space?.id });
    app.memory.recordMcpCall({ clientId: beta?.id ?? "", tool: "read_page", outcome: "denied", summary: "Roadmap" });
    app.memory.recordMcpCall({ clientId: alpha?.id ?? "", tool: "create_page", outcome: "error", summary: "New page" });
    return { app, alpha, beta };
  }

  const audit = () => screen.getByRole("group", { name: t("mcp.audit.title") });
  const bodyRows = () => within(audit()).getAllByRole("row").slice(1);

  it("lists the last calls, newest first, with who, what, on what and how it ended", async () => {
    await withCalls();
    await waitFor(() => {
      expect(bodyRows()).toHaveLength(3);
    });
    const rows = bodyRows();
    expect(within(rows[0] as HTMLElement).getAllByRole("cell").map((cell) => cell.textContent)).toEqual([
      expect.any(String),
      "Alpha",
      "create_page",
      "New page",
      t("mcp.audit.outcome.error"),
    ]);
    const cells = (row: HTMLElement | undefined) => within(row as HTMLElement).getAllByRole("cell").map((cell) => cell.textContent);
    expect(cells(rows[1])).toEqual([expect.any(String), "Beta", "read_page", "Roadmap", t("mcp.audit.outcome.denied")]);
    expect(cells(rows[2])).toEqual([expect.any(String), "Alpha", "search", "tomato Personal", t("mcp.audit.outcome.ok")]);
    expect(rows[1]).toHaveAttribute("data-outcome", "denied");
    expect(within(rows[0] as HTMLElement).getByText(t("mcp.audit.outcome.error"))).toHaveClass("mcp-outcome-problem");
    expect(rows[0]?.querySelector("time")).toHaveAttribute("datetime");
  });

  it("filters by client and goes back to all", async () => {
    const { app } = await withCalls();
    await waitFor(() => {
      expect(bodyRows()).toHaveLength(3);
    });
    await app.user.selectOptions(within(audit()).getByLabelText(t("mcp.audit.filter")), "Beta");
    await waitFor(() => {
      expect(bodyRows()).toHaveLength(1);
    });
    expect(within(bodyRows()[0] as HTMLElement).getByText("read_page")).toBeInTheDocument();
    await app.user.selectOptions(within(audit()).getByLabelText(t("mcp.audit.filter")), t("mcp.audit.allClients"));
    await waitFor(() => {
      expect(bodyRows()).toHaveLength(3);
    });
  });

  it("clears the log", async () => {
    const { app } = await withCalls();
    await waitFor(() => {
      expect(bodyRows()).toHaveLength(3);
    });
    await app.user.click(within(audit()).getByRole("button", { name: t("mcp.audit.clear") }));
    expect(await within(audit()).findByText(t("mcp.audit.none"))).toBeInTheDocument();
    expect(within(audit()).getByRole("button", { name: t("mcp.audit.clear") })).toBeDisabled();
    const rows = await app.memory.dispatch({ type: "McpAuditList", data: {} });
    expect(rows).toMatchObject({ data: [] });
  });

  it("follows a call that arrives while the screen is open (the read waits for the second to pass)", async () => {
    const { app, alpha } = await withCalls();
    await waitFor(() => {
      expect(bodyRows()).toHaveLength(3);
    });
    act(() => {
      app.memory.recordMcpCall({ clientId: alpha?.id ?? "", tool: "list_spaces", outcome: "ok" });
    });
    await waitFor(
      () => {
        expect(bodyRows()).toHaveLength(4);
      },
      { timeout: 4000 },
    );
    expect(bodyRows()[0]).toHaveTextContent("list_spaces");
  });
});
