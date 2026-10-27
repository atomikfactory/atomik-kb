import type { MemoryTransport } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { t } from "./i18n";
import { NOW, openScreen, renderApp } from "./test/render";

async function openSettings(options: Parameters<typeof renderApp>[0] = {}) {
  const app = renderApp({ workspaceName: "Research", ...options });
  await openScreen(app.user, "nav.settings");
  await screen.findByRole("heading", { name: t("settings.title") });
  return app;
}

const themeRadio = (name: "light" | "dark" | "system") =>
  screen.getByRole("radio", { name: t(`settings.theme.${name}`) });

describe("settings: theme", () => {
  it("defaults to matching the system when nothing is stored", async () => {
    await openSettings();
    expect(themeRadio("system")).toBeChecked();
    expect(themeRadio("light")).not.toBeChecked();
    expect(themeRadio("dark")).not.toBeChecked();
    expect(document.documentElement).toHaveAttribute("data-theme", "system");
  });

  it("shows and applies the stored theme", async () => {
    await openSettings({ settings: { "ui.theme": "dark" } });
    expect(themeRadio("dark")).toBeChecked();
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
  });

  it("falls back to system for a stored value it does not understand", async () => {
    await openSettings({ settings: { "ui.theme": "solarized" } });
    expect(themeRadio("system")).toBeChecked();
  });

  it("persists a choice through the transport and applies it to the document", async () => {
    const { user, memory } = await openSettings();
    await user.click(themeRadio("dark"));

    await waitFor(() => {
      expect(themeRadio("dark")).toBeChecked();
    });
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
    // The core, not just the UI, holds the new value.
    await expect(memory.dispatch({ type: "SettingsGet", data: { key: "ui.theme" } })).resolves.toEqual({
      type: "Setting",
      data: "dark",
    });

    await user.click(themeRadio("light"));
    await waitFor(() => {
      expect(document.documentElement).toHaveAttribute("data-theme", "light");
    });
    await expect(memory.dispatch({ type: "SettingsGet", data: { key: "ui.theme" } })).resolves.toEqual({
      type: "Setting",
      data: "light",
    });
  });

  it("keeps the previous theme and says so when saving fails", async () => {
    const { user, memory } = await openSettings({ settings: { "ui.theme": "light" } });
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    await user.click(themeRadio("dark"));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("settings.theme.error", { message: "database error: disk I/O error" }),
    );
    expect(themeRadio("light")).toBeChecked();
    expect(document.documentElement).toHaveAttribute("data-theme", "light");
  });

  it("follows a theme written by someone else, through the SettingChanged event", async () => {
    const { memory } = await openSettings();
    await memory.dispatch({ type: "SettingsSet", data: { key: "ui.theme", value: "dark" } });
    await waitFor(() => {
      expect(themeRadio("dark")).toBeChecked();
    });
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
  });

  it("applies the stored theme from the first screen on, before settings is opened", async () => {
    renderApp({ workspaceName: "Research", settings: { "ui.theme": "dark" } });
    await screen.findByRole("navigation", { name: t("nav.label") });
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
  });
});

const ACCENT_NAMES = ["gray", "brown", "orange", "yellow", "green", "blue", "purple", "pink", "red"] as const;

const accentGroup = () => screen.getByRole("radiogroup", { name: t("settings.accent.legend") });
const accentRadios = () => within(accentGroup()).getAllByRole("radio");
const accentRadio = (name: (typeof ACCENT_NAMES)[number]) =>
  within(accentGroup()).getByRole("radio", { name: t(`callout.color.${name}`) });
const storedAccent = (memory: MemoryTransport) => memory.dispatch({ type: "SettingsGet", data: { key: "ui.accent" } });

describe("settings: accent colour", () => {
  afterEach(() => {
    delete document.documentElement.dataset.accent;
  });

  it("offers the nine palette colours as a radio group, named like the palette, with blue checked by default", async () => {
    await openSettings();
    const radios = accentRadios();
    expect(radios.map((radio) => radio.getAttribute("aria-label"))).toEqual(
      ACCENT_NAMES.map((name) => t(`callout.color.${name}`)),
    );
    expect(radios.every((radio) => radio.getAttribute("role") === "radio")).toBe(true);
    for (const name of ACCENT_NAMES) {
      expect(accentRadio(name)).toHaveAttribute("aria-checked", name === "blue" ? "true" : "false");
      expect(accentRadio(name)).toHaveAttribute("data-accent-swatch", name);
    }
    // The default is the absence of the setting and of the attribute.
    expect(document.documentElement).not.toHaveAttribute("data-accent");
  });

  it("does not write the setting just by opening Settings", async () => {
    const { memory } = await openSettings();
    await expect(storedAccent(memory)).resolves.toEqual({ type: "Setting", data: null });
  });

  it("shows and applies the stored accent", async () => {
    await openSettings({ settings: { "ui.accent": "green" } });
    expect(accentRadio("green")).toHaveAttribute("aria-checked", "true");
    expect(accentRadio("blue")).toHaveAttribute("aria-checked", "false");
    expect(document.documentElement).toHaveAttribute("data-accent", "green");
  });

  it("falls back to blue, and no attribute, for a stored value it does not understand", async () => {
    await openSettings({ settings: { "ui.accent": "teal" } });
    expect(accentRadio("blue")).toHaveAttribute("aria-checked", "true");
    expect(document.documentElement).not.toHaveAttribute("data-accent");
  });

  it("applies the stored accent from the first screen on, before Settings is opened", async () => {
    renderApp({ workspaceName: "Research", settings: { "ui.accent": "purple", "ui.theme": "dark" } });
    await screen.findByRole("navigation", { name: t("nav.label") });
    expect(document.documentElement).toHaveAttribute("data-accent", "purple");
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
  });

  it("persists a choice through the transport and applies it to the document", async () => {
    const { user, memory } = await openSettings();
    await user.click(accentRadio("red"));

    await waitFor(() => {
      expect(accentRadio("red")).toHaveAttribute("aria-checked", "true");
    });
    expect(accentRadio("blue")).toHaveAttribute("aria-checked", "false");
    expect(document.documentElement).toHaveAttribute("data-accent", "red");
    await expect(storedAccent(memory)).resolves.toEqual({ type: "Setting", data: "red" });

    // Choosing blue again goes back to the default look, with no attribute.
    await user.click(accentRadio("blue"));
    await waitFor(() => {
      expect(document.documentElement).not.toHaveAttribute("data-accent");
    });
    expect(accentRadio("blue")).toHaveAttribute("aria-checked", "true");
    await expect(storedAccent(memory)).resolves.toEqual({ type: "Setting", data: "blue" });
  });

  it("is one tab stop, on the checked colour, and the arrow keys move and choose, wrapping around", async () => {
    const { user, memory } = await openSettings({ settings: { "ui.accent": "orange" } });
    expect(accentRadios().filter((radio) => radio.tabIndex === 0)).toEqual([accentRadio("orange")]);

    act(() => {
      accentRadio("orange").focus();
    });
    await user.keyboard("{ArrowRight}");
    await waitFor(() => {
      expect(accentRadio("yellow")).toHaveAttribute("aria-checked", "true");
    });
    expect(accentRadio("yellow")).toHaveFocus();
    expect(document.documentElement).toHaveAttribute("data-accent", "yellow");
    expect(accentRadios().filter((radio) => radio.tabIndex === 0)).toEqual([accentRadio("yellow")]);

    await user.keyboard("{ArrowDown}");
    await waitFor(() => {
      expect(accentRadio("green")).toHaveAttribute("aria-checked", "true");
    });
    await user.keyboard("{ArrowLeft}{ArrowUp}");
    await waitFor(() => {
      expect(accentRadio("orange")).toHaveAttribute("aria-checked", "true");
    });
    await expect(storedAccent(memory)).resolves.toEqual({ type: "Setting", data: "orange" });

    // Gray is first and red last: the keys wrap.
    await user.click(accentRadio("gray"));
    await user.keyboard("{ArrowLeft}");
    await waitFor(() => {
      expect(accentRadio("red")).toHaveAttribute("aria-checked", "true");
    });
    expect(accentRadio("red")).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    await waitFor(() => {
      expect(accentRadio("gray")).toHaveAttribute("aria-checked", "true");
    });
  });

  it("can be chosen with the space bar", async () => {
    const { user } = await openSettings();
    act(() => {
      accentRadio("pink").focus();
    });
    await user.keyboard(" ");
    await waitFor(() => {
      expect(accentRadio("pink")).toHaveAttribute("aria-checked", "true");
    });
    expect(document.documentElement).toHaveAttribute("data-accent", "pink");
  });

  it("keeps the previous accent and says so when saving fails", async () => {
    const { user, memory } = await openSettings({ settings: { "ui.accent": "green" } });
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    await user.click(accentRadio("pink"));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("settings.accent.error", { message: "database error: disk I/O error" }),
    );
    expect(accentRadio("green")).toHaveAttribute("aria-checked", "true");
    expect(accentRadio("pink")).toHaveAttribute("aria-checked", "false");
    expect(document.documentElement).toHaveAttribute("data-accent", "green");
  });

  it("follows an accent written by someone else, through the SettingChanged event", async () => {
    const { memory } = await openSettings();
    await memory.dispatch({ type: "SettingsSet", data: { key: "ui.accent", value: "brown" } });
    await waitFor(() => {
      expect(accentRadio("brown")).toHaveAttribute("aria-checked", "true");
    });
    expect(document.documentElement).toHaveAttribute("data-accent", "brown");
  });

  it("sits beside the theme, in Appearance, and does not touch the theme", async () => {
    const { user, memory } = await openSettings({ settings: { "ui.theme": "dark" } });
    const appearance = screen.getByRole("region", { name: t("settings.appearance") });
    expect(within(appearance).getByRole("radiogroup", { name: t("settings.accent.legend") })).toBe(accentGroup());
    expect(within(appearance).getByRole("radio", { name: t("settings.theme.dark") })).toBeChecked();
    await user.click(accentRadio("purple"));
    await waitFor(() => {
      expect(document.documentElement).toHaveAttribute("data-accent", "purple");
    });
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
    await expect(memory.dispatch({ type: "SettingsGet", data: { key: "ui.theme" } })).resolves.toEqual({
      type: "Setting",
      data: "dark",
    });
  });
});

describe("settings: device", () => {
  it("shows the device name, platform and first launch time", async () => {
    await openSettings({ status: { platform: "windows" }, deviceName: "Desk PC" });
    expect(screen.getByLabelText(t("settings.device.nameLabel"))).toHaveValue("Desk PC");
    const device = screen.getByRole("region", { name: t("settings.device") });
    expect(within(device).getByText(t("settings.device.platform")).nextElementSibling).toHaveTextContent("windows");
    const launched = device.querySelector("time");
    expect(launched).toHaveAttribute("datetime", new Date(NOW).toISOString());
  });

  it("renames the device through the transport and confirms", async () => {
    const { user, memory } = await openSettings({ deviceName: "Desk PC" });
    const input = screen.getByLabelText(t("settings.device.nameLabel"));
    const save = screen.getByRole("button", { name: t("settings.device.save") });
    expect(save).toBeDisabled();

    await user.clear(input);
    await user.type(input, "  Work laptop ");
    expect(save).toBeEnabled();
    await user.click(save);

    expect(await screen.findByText(t("settings.device.saved"))).toBeInTheDocument();
    // The core trimmed the name and the form shows what was stored.
    await waitFor(() => {
      expect(input).toHaveValue("Work laptop");
    });
    expect(screen.getByRole("button", { name: t("settings.device.save") })).toBeDisabled();
    await expect(memory.dispatch({ type: "DeviceInfo" })).resolves.toMatchObject({
      data: { name: "Work laptop" },
    });
  });

  it("does not offer to save an unchanged name, and the confirmation goes away when editing again", async () => {
    const { user } = await openSettings({ deviceName: "Desk PC" });
    const input = screen.getByLabelText(t("settings.device.nameLabel"));
    await user.type(input, "!");
    await user.click(screen.getByRole("button", { name: t("settings.device.save") }));
    await screen.findByText(t("settings.device.saved"));
    await user.type(input, "?");
    expect(screen.queryByText(t("settings.device.saved"))).not.toBeInTheDocument();
  });

  it("rejects a blank name without bothering the core", async () => {
    const { user } = await openSettings({ deviceName: "Desk PC" });
    const input = screen.getByLabelText(t("settings.device.nameLabel"));
    await user.clear(input);
    await user.type(input, "   ");
    await user.click(screen.getByRole("button", { name: t("settings.device.save") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("settings.device.error.empty"));
    expect(input).toHaveAttribute("aria-invalid", "true");
  });

  it("shows the core's validation message for an over-long name", async () => {
    const { user } = await openSettings({ deviceName: "Desk PC" });
    const input = screen.getByLabelText(t("settings.device.nameLabel"));
    await user.clear(input);
    await user.click(input);
    await user.paste("x".repeat(101));
    await user.click(screen.getByRole("button", { name: t("settings.device.save") }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Name must be at most 100 characters.");
    expect(input).toHaveValue("x".repeat(101));
  });

  it("explains other failures", async () => {
    const { user, memory } = await openSettings({ deviceName: "Desk PC" });
    const input = screen.getByLabelText(t("settings.device.nameLabel"));
    await user.type(input, "!");
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    await user.click(screen.getByRole("button", { name: t("settings.device.save") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("settings.device.error.generic", { message: "database error: disk I/O error" }),
    );
  });

  it("follows a rename made elsewhere (DeviceRenamed event)", async () => {
    const { memory } = await openSettings({ deviceName: "Desk PC" });
    await memory.dispatch({ type: "DeviceRename", data: { name: "Renamed elsewhere" } });
    await waitFor(() => {
      expect(screen.getByLabelText(t("settings.device.nameLabel"))).toHaveValue("Renamed elsewhere");
    });
  });
});

describe("settings: data", () => {
  it("shows where the data lives and whether the database is encrypted", async () => {
    await openSettings({
      status: { dataDir: "C:\\Users\\me\\AppData\\Roaming\\com.atomik-kb.desktop", dbEncrypted: true },
    });
    const data = screen.getByRole("region", { name: t("settings.data") });
    expect(within(data).getByText(t("settings.data.location")).nextElementSibling).toHaveTextContent(
      "C:\\Users\\me\\AppData\\Roaming\\com.atomik-kb.desktop",
    );
    expect(within(data).getByText(t("settings.data.encrypted")).nextElementSibling).toHaveTextContent(
      t("common.yes"),
    );
  });

  it("says so plainly when the database is not encrypted", async () => {
    await openSettings({ status: { dbEncrypted: false } });
    const data = screen.getByRole("region", { name: t("settings.data") });
    expect(within(data).getByText(t("settings.data.encrypted")).nextElementSibling).toHaveTextContent(
      t("common.no"),
    );
  });
});
