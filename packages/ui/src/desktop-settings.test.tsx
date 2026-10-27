// Settings -> Desktop (ADR-0040): the section exists only with a desktop shell; "Keep running in the tray when the
// window is closed" writes `app.tray.close_to_tray`; the quick-capture shortcut is recorded by pressing the keys in its
// field, written to `app.shortcut.capture`, refused when it has no modifier, cleared or reset, and Settings says
// what the shell reported about the registration (another program may own the combination).

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { ShellEvent, ShortcutState } from "@atomik-kb/core-client";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import { CAPTURE_SHORTCUT_KEY, CLOSE_TO_TRAY_KEY } from "./store-shell";
import { NOW, openScreen, renderApp, renderWith } from "./test/render";
import { withShell } from "./test/shell";
import type { ShellTransport } from "./test/shell";

afterEach(() => {
  vi.restoreAllMocks();
});

/** The app with a desktop shell around the in-memory core, on the Settings screen. */
async function settings(options: { shortcut?: ShortcutState; stored?: Record<string, boolean | string> } = {}) {
  const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", settings: options.stored ?? {} });
  const shell = withShell(memory, options.shortcut === undefined ? {} : { shortcut: options.shortcut });
  const app = renderWith(shell.transport);
  await openScreen(app.user, "nav.settings");
  return { ...app, memory, shell, client: new CoreClient(memory) };
}

const section = () => screen.findByRole("region", { name: t("settings.desktop") });
const trayToggle = () => screen.findByRole("switch", { name: t("settings.desktop.tray.label") });
const field = () => screen.findByRole<HTMLInputElement>("textbox", { name: t("settings.desktop.shortcut.label") });

/** One key press in the field, as the keyboard sends it. */
function press(target: HTMLElement, code: string, modifiers: { ctrl?: boolean; alt?: boolean; shift?: boolean; meta?: boolean } = {}) {
  fireEvent.keyDown(target, {
    code,
    key: code,
    ctrlKey: modifiers.ctrl === true,
    altKey: modifiers.alt === true,
    shiftKey: modifiers.shift === true,
    metaKey: modifiers.meta === true,
  });
}

async function stored(client: CoreClient, key: string) {
  return client.settingsGet(key);
}

describe("the Desktop section", () => {
  it("is on the Settings screen with a desktop shell", async () => {
    await settings();
    const desktop = await section();
    expect(within(desktop).getByRole("heading", { name: t("settings.desktop"), level: 2 })).toBeInTheDocument();
    expect(within(desktop).getByRole("switch", { name: t("settings.desktop.tray.label") })).toBeInTheDocument();
    expect(within(desktop).getByRole("group", { name: t("settings.desktop.shortcut.legend") })).toBeInTheDocument();
  });

  it("is not on the Settings screen of the in-memory dev UI or the web build", async () => {
    const { user } = renderApp({ workspaceName: "Research" });
    await openScreen(user, "nav.settings");
    await screen.findByRole("heading", { name: t("settings.title") });
    expect(screen.queryByRole("heading", { name: t("settings.desktop") })).not.toBeInTheDocument();
    expect(screen.queryByRole("switch", { name: t("settings.desktop.tray.label") })).not.toBeInTheDocument();
    expect(screen.queryByText(t("settings.desktop.shortcut.legend"))).not.toBeInTheDocument();
  });
});

describe("keep running in the tray", () => {
  it("is on until the setting says false", async () => {
    await settings();
    expect(await trayToggle()).toBeChecked();
  });

  it("shows a setting that was stored as false", async () => {
    await settings({ stored: { [CLOSE_TO_TRAY_KEY]: false } });
    await waitFor(async () => {
      expect(await trayToggle()).not.toBeChecked();
    });
  });

  it("writes the setting when it is turned off and on", async () => {
    const { user, client } = await settings();
    const toggle = await trayToggle();

    await user.click(toggle);
    await waitFor(() => {
      expect(toggle).not.toBeChecked();
    });
    expect(await stored(client, CLOSE_TO_TRAY_KEY)).toBe(false);

    await user.click(toggle);
    await waitFor(() => {
      expect(toggle).toBeChecked();
    });
    expect(await stored(client, CLOSE_TO_TRAY_KEY)).toBe(true);
  });

  it("follows a change that was made elsewhere", async () => {
    const { client } = await settings();
    const toggle = await trayToggle();
    await act(async () => {
      await client.settingsSet(CLOSE_TO_TRAY_KEY, false);
    });
    await waitFor(() => {
      expect(toggle).not.toBeChecked();
    });
  });

  it("says why when the setting could not be saved", async () => {
    const { user, memory } = await settings();
    const toggle = await trayToggle();
    const dispatch = memory.dispatch.bind(memory);
    vi.spyOn(memory, "dispatch").mockImplementation((request) =>
      request.type === "SettingsSet" ? Promise.reject(new Error("the disk is full")) : dispatch(request),
    );
    await user.click(toggle);
    expect(await screen.findByRole("alert")).toHaveTextContent(t("settings.desktop.tray.error", { message: "the disk is full" }));
    expect(toggle).toBeChecked();
  });

  it("explains that reminders keep firing", async () => {
    await settings();
    const toggle = await trayToggle();
    expect(toggle).toHaveAccessibleDescription(t("settings.desktop.tray.hint"));
  });
});

describe("the quick capture shortcut", () => {
  it("shows the default while nothing is stored", async () => {
    await settings();
    await waitFor(async () => {
      expect(await field()).toHaveValue("Ctrl+Alt+N");
    });
  });

  it("shows what is stored, and none for the empty text", async () => {
    const { unmount } = await settings({ stored: { [CAPTURE_SHORTCUT_KEY]: "Ctrl+Shift+K" } });
    await waitFor(async () => {
      expect(await field()).toHaveValue("Ctrl+Shift+K");
    });
    unmount();
    await settings({ stored: { [CAPTURE_SHORTCUT_KEY]: "" } });
    await waitFor(async () => {
      expect(await field()).toHaveValue(t("settings.desktop.shortcut.none"));
    });
  });

  it("records the keys pressed while it has focus and writes the accelerator", async () => {
    const { user, client } = await settings();
    const input = await field();
    await user.click(input);
    expect(input).toHaveValue(t("settings.desktop.shortcut.recording"));

    press(input, "KeyK", { ctrl: true, shift: true });

    await waitFor(async () => {
      expect(await stored(client, CAPTURE_SHORTCUT_KEY)).toBe("Ctrl+Shift+K");
    });
    // Still recording until the field loses focus; then it shows the shortcut.
    await user.tab();
    await waitFor(() => {
      expect(input).toHaveValue("Ctrl+Shift+K");
    });
  });

  it("writes the shell's spelling whatever the order the modifiers were pressed in", async () => {
    const { user, client } = await settings();
    const input = await field();
    await user.click(input);
    press(input, "Space", { meta: true, alt: true });
    await waitFor(async () => {
      expect(await stored(client, CAPTURE_SHORTCUT_KEY)).toBe("Alt+Super+Space");
    });
  });

  it("waits while only modifiers are down", async () => {
    const { user, client } = await settings();
    const input = await field();
    await user.click(input);
    press(input, "ControlLeft", { ctrl: true });
    press(input, "ShiftLeft", { ctrl: true, shift: true });
    await act(async () => {
      await Promise.resolve();
    });
    expect(await stored(client, CAPTURE_SHORTCUT_KEY)).toBeNull();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("refuses a key without Ctrl, Alt or Super and writes nothing", async () => {
    const { user, client } = await settings();
    const input = await field();
    await user.click(input);

    for (const [code, modifiers] of [
      ["KeyN", {}],
      ["F5", {}],
      ["KeyA", { shift: true }],
    ] as const) {
      press(input, code, modifiers);
      expect(await screen.findByRole("alert")).toHaveTextContent(t("settings.desktop.shortcut.error.noModifier"));
    }
    expect(await stored(client, CAPTURE_SHORTCUT_KEY)).toBeNull();
    expect(input).toHaveAttribute("aria-invalid", "true");

    // A valid combination after it is taken, and the message goes.
    press(input, "KeyN", { ctrl: true });
    await waitFor(() => {
      expect(screen.queryByText(t("settings.desktop.shortcut.error.noModifier"))).not.toBeInTheDocument();
    });
    await waitFor(async () => {
      expect(await stored(client, CAPTURE_SHORTCUT_KEY)).toBe("Ctrl+N");
    });
  });

  it("refuses a key the shell does not accept", async () => {
    const { user, client } = await settings();
    const input = await field();
    await user.click(input);
    press(input, "Numpad1", { ctrl: true });
    expect(await screen.findByRole("alert")).toHaveTextContent(t("settings.desktop.shortcut.error.unsupported"));
    expect(await stored(client, CAPTURE_SHORTCUT_KEY)).toBeNull();
  });

  it("does not type the key into the page: the press is the field's", async () => {
    const { user } = await settings();
    const input = await field();
    await user.click(input);
    const stop = fireEvent.keyDown(input, { code: "KeyK", key: "k", ctrlKey: true });
    expect(stop).toBe(false); // preventDefault was called
  });

  it("lets Tab and Escape leave the field as they leave any other", async () => {
    const { user } = await settings();
    const input = await field();
    await user.click(input);
    expect(input).toHaveFocus();
    expect(fireEvent.keyDown(input, { code: "Tab", key: "Tab" })).toBe(true); // not prevented: focus may move
    await user.keyboard("{Escape}");
    expect(input).not.toHaveFocus();
    expect(input).not.toHaveValue(t("settings.desktop.shortcut.recording"));
  });

  it("clears the shortcut with Clear, and Clear is off when there is none", async () => {
    const { user, client } = await settings();
    const clear = await screen.findByRole("button", { name: t("settings.desktop.shortcut.clear") });
    await waitFor(() => {
      expect(clear).toBeEnabled();
    });
    await user.click(clear);

    await waitFor(async () => {
      expect(await stored(client, CAPTURE_SHORTCUT_KEY)).toBe("");
    });
    await waitFor(async () => {
      expect(await field()).toHaveValue(t("settings.desktop.shortcut.none"));
    });
    expect(clear).toBeDisabled();
  });

  it("resets to the default with Reset, and Reset is off when it already is", async () => {
    const { user, client } = await settings({ stored: { [CAPTURE_SHORTCUT_KEY]: "Alt+Space" } });
    const reset = await screen.findByRole("button", { name: t("settings.desktop.shortcut.reset") });
    await waitFor(() => {
      expect(reset).toBeEnabled();
    });
    await user.click(reset);

    await waitFor(async () => {
      expect(await stored(client, CAPTURE_SHORTCUT_KEY)).toBe("Ctrl+Alt+N");
    });
    await waitFor(async () => {
      expect(await field()).toHaveValue("Ctrl+Alt+N");
    });
    expect(reset).toBeDisabled();
  });

  it("says why when the shortcut could not be saved", async () => {
    const { user, memory } = await settings();
    const input = await field();
    const dispatch = memory.dispatch.bind(memory);
    vi.spyOn(memory, "dispatch").mockImplementation((request) =>
      request.type === "SettingsSet" ? Promise.reject(new Error("the disk is full")) : dispatch(request),
    );
    await user.click(input);
    press(input, "KeyK", { ctrl: true });
    expect(await screen.findByRole("alert")).toHaveTextContent(t("settings.desktop.shortcut.error.save", { message: "the disk is full" }));
  });
});

describe("what the shell says about the shortcut", () => {
  const MESSAGES: Record<ShortcutState, { text: string; role: "alert" | "status" }> = {
    registered: { text: t("settings.desktop.shortcut.state.registered"), role: "status" },
    disabled: { text: t("settings.desktop.shortcut.state.disabled"), role: "status" },
    taken: { text: t("settings.desktop.shortcut.state.taken"), role: "alert" },
    invalid: { text: t("settings.desktop.shortcut.state.invalid"), role: "alert" },
  };

  it.each(Object.entries(MESSAGES) as [ShortcutState, { text: string; role: "alert" | "status" }][])(
    "shows the state %s from the shell",
    async (state, { text, role }) => {
      await settings({ shortcut: state });
      const message = await within(await section()).findByText(text);
      expect(message).toHaveAttribute("role", role);
    },
  );

  it("says that another app uses the shortcut, and marks the field", async () => {
    await settings({ shortcut: "taken" });
    expect(await screen.findByRole("alert")).toHaveTextContent("This shortcut is used by another app");
    expect(await field()).toHaveAttribute("aria-invalid", "true");
  });

  it("says nothing before the shell has", async () => {
    await settings();
    await field();
    for (const message of Object.values(MESSAGES)) {
      expect(screen.queryByText(message.text)).not.toBeInTheDocument();
    }
    expect(await field()).not.toHaveAttribute("aria-invalid", "true");
  });

  it("follows the shell when it registers again after a change", async () => {
    const { shell } = await settings({ shortcut: "taken" });
    await screen.findByText(MESSAGES.taken.text);
    emit(shell, { type: "shortcut", data: { state: "registered" } });
    expect(await screen.findByText(MESSAGES.registered.text)).toBeInTheDocument();
    expect(screen.queryByText(MESSAGES.taken.text)).not.toBeInTheDocument();
    expect(await field()).not.toHaveAttribute("aria-invalid", "true");
  });
});

function emit(shell: ShellTransport, event: ShellEvent): void {
  act(() => {
    shell.emit(event);
  });
}
