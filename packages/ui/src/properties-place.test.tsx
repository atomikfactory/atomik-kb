// The place editor and the computed properties of the panel (ADR-0026, amendment of 2026-10-08):
// what the fields commit and refuse, the "Open in map" link, the rows of the computed kinds and
// the device names.

import { within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { SeedSpace } from "@atomik-kb/core-client";

import { t } from "./i18n";
import { deviceLabel, parsePlaceDraft, placeDraftOf, placeMapUrl } from "./property-format";
import { openProperties, propertyRow, renderProperties, rowNames, valueOf } from "./test/properties";

const SPACE: SeedSpace = {
  name: "Personal",
  properties: [
    { name: "Where", kind: "place" },
    { name: "Created", kind: "created_time" },
    { name: "Edited", kind: "last_edited_time" },
    { name: "Made by", kind: "created_by" },
    { name: "Edited by", kind: "last_edited_by" },
  ],
  pages: [
    { title: "Cafe", properties: { Where: { name: "Café de Flore, Paris", lat: 48.854, lon: 2.333 } } },
    { title: "Home", properties: { Where: { name: "Home office" } } },
    { title: "Plain" },
  ],
};

describe("placeMapUrl", () => {
  it("opens the position on OpenStreetMap, or a search for the name", () => {
    expect(placeMapUrl({ name: "Café", lat: 48.854, lon: 2.333 })).toBe(
      "https://www.openstreetmap.org/?mlat=48.854&mlon=2.333#map=16/48.854/2.333",
    );
    expect(placeMapUrl({ name: "Café de Flore & Co" })).toBe(
      "https://www.openstreetmap.org/search?query=Caf%C3%A9%20de%20Flore%20%26%20Co",
    );
  });
});

describe("parsePlaceDraft", () => {
  it("reads the texts of the fields", () => {
    expect(parsePlaceDraft({ name: "  ", lat: "", lon: "" })).toEqual({ ok: true, value: null });
    expect(parsePlaceDraft({ name: " Home ", lat: "", lon: "" })).toEqual({
      ok: true,
      value: { kind: "place", value: { name: "Home" } },
    });
    expect(parsePlaceDraft({ name: "Pole", lat: "-90", lon: "180" })).toEqual({
      ok: true,
      value: { kind: "place", value: { name: "Pole", lat: -90, lon: 180 } },
    });
    expect(parsePlaceDraft({ name: "", lat: "1", lon: "2" })).toEqual({
      ok: false,
      problem: t("properties.place.problem.name"),
    });
    expect(parsePlaceDraft({ name: "x", lat: "1", lon: "" })).toEqual({
      ok: false,
      problem: t("properties.place.problem.pair"),
    });
    expect(parsePlaceDraft({ name: "x", lat: "91", lon: "2" })).toEqual({
      ok: false,
      problem: t("properties.place.problem.lat"),
    });
    expect(parsePlaceDraft({ name: "x", lat: "1", lon: "abc" })).toEqual({
      ok: false,
      problem: t("properties.place.problem.lon"),
    });
    expect(parsePlaceDraft({ name: "a\nb", lat: "", lon: "" }).ok).toBe(false);
  });

  it("starts from a value", () => {
    expect(placeDraftOf(undefined)).toEqual({ name: "", lat: "", lon: "" });
    expect(placeDraftOf({ kind: "place", value: { name: "A", lat: 1.5, lon: -2 } })).toEqual({
      name: "A",
      lat: "1.5",
      lon: "-2",
    });
  });
});

describe("deviceLabel", () => {
  const own = { deviceId: "d1", name: "Desk" };
  const log = [
    { deviceId: "d2", name: "Phone", addedAt: 1, addedViaKit: false, isThisDevice: false, signingKey: "" },
    { deviceId: "d3", name: "Old", addedAt: 1, revokedAt: 5, addedViaKit: false, isThisDevice: false, signingKey: "" },
  ];
  it("names this device, the others by the log, and an unknown or revoked one as unknown", () => {
    expect(deviceLabel("d1", [], own)).toBe("Desk");
    expect(deviceLabel("d2", log, own)).toBe("Phone");
    expect(deviceLabel("d3", log, own)).toBe(t("properties.computed.unknownDevice"));
    expect(deviceLabel("d9", log, own)).toBe(t("properties.computed.unknownDevice"));
    expect(deviceLabel("d9", [], null)).toBe(t("properties.computed.unknownDevice"));
  });
});

describe("the place editor", () => {
  it("shows the fields, commits all three as one value and opens the map through the shell", async () => {
    const openExternal = vi.fn(() => Promise.resolve());
    const { user, memory } = await renderProperties([SPACE], { openExternal });
    const panel = await openProperties(user, "Cafe");
    const row = propertyRow(panel, "Where");
    const name = within(row).getByRole("textbox", { name: "Where" });
    expect(name).toHaveValue("Café de Flore, Paris");
    expect(within(row).getByRole("textbox", { name: t("properties.place.lat") })).toHaveValue("48.854");
    expect(within(row).getByRole("textbox", { name: t("properties.place.lon") })).toHaveValue("2.333");

    await user.click(
      within(row).getByRole("button", { name: t("properties.place.openMap", { name: "Café de Flore, Paris" }) }),
    );
    expect(openExternal).toHaveBeenCalledWith("https://www.openstreetmap.org/?mlat=48.854&mlon=2.333#map=16/48.854/2.333");

    // Moving between the fields saves nothing; leaving the editor saves once.
    await user.clear(name);
    await user.type(name, "Louvre");
    const lat = within(row).getByRole("textbox", { name: t("properties.place.lat") });
    await user.clear(lat);
    await user.type(lat, "48.8606");
    expect(await valueOf(memory, "Cafe", "Where")).toEqual({
      kind: "place",
      value: { name: "Café de Flore, Paris", lat: 48.854, lon: 2.333 },
    });
    await user.type(lat, "{Enter}");
    await vi.waitFor(async () => {
      expect(await valueOf(memory, "Cafe", "Where")).toEqual({
        kind: "place",
        value: { name: "Louvre", lat: 48.8606, lon: 2.333 },
      });
    });
  });

  it("opens a search for the name when there are no coordinates, and refuses what the core would", async () => {
    const openExternal = vi.fn(() => Promise.resolve());
    const { user, memory } = await renderProperties([SPACE], { openExternal });
    const panel = await openProperties(user, "Home");
    const row = propertyRow(panel, "Where");
    await user.click(within(row).getByRole("button", { name: t("properties.place.openMap", { name: "Home office" }) }));
    expect(openExternal).toHaveBeenCalledWith("https://www.openstreetmap.org/search?query=Home%20office");

    const lat = within(row).getByRole("textbox", { name: t("properties.place.lat") });
    await user.type(lat, "12{Enter}");
    expect(await within(panel).findByText(t("properties.place.problem.pair"))).toBeInTheDocument();
    expect(await valueOf(memory, "Home", "Where")).toEqual({ kind: "place", value: { name: "Home office" } });

    // Escape puts the fields back.
    await user.keyboard("{Escape}");
    expect(lat).toHaveValue("");
    expect(within(panel).queryByText(t("properties.place.problem.pair"))).not.toBeInTheDocument();
  });

  it("clears the value when the fields are emptied, and is offered for a page that has none", async () => {
    const { user, memory } = await renderProperties([SPACE]);
    const panel = await openProperties(user, "Home");
    const name = within(propertyRow(panel, "Where")).getByRole("textbox", { name: "Where" });
    await user.clear(name);
    await user.type(name, "{Enter}");
    await vi.waitFor(async () => {
      expect(await valueOf(memory, "Home", "Where")).toBeUndefined();
    });

    const plain = await openProperties(user, "Plain");
    expect(rowNames(plain)).not.toContain("Where");
    await user.click(within(plain).getByRole("button", { name: t("properties.add") }));
    await user.click(await within(document.body).findByRole("menuitem", { name: /Where/u }));
    const field = await within(plain).findByRole("textbox", { name: "Where" });
    await user.type(field, "Lisbon{Enter}");
    await vi.waitFor(async () => {
      expect(await valueOf(memory, "Plain", "Where")).toEqual({ kind: "place", value: { name: "Lisbon" } });
    });
  });
});

describe("the computed properties", () => {
  it("show on every page, read only, with the device named", async () => {
    const { user } = await renderProperties([SPACE]);
    const panel = await openProperties(user, "Plain");
    expect(rowNames(panel)).toEqual(["Tags", "Created", "Edited", "Made by", "Edited by"]);
    for (const name of ["Created", "Edited"]) {
      const text = within(propertyRow(panel, name)).getByLabelText(new RegExp(`^${name}: `, "u"));
      expect(text.textContent).not.toBe("");
      expect(within(propertyRow(panel, name)).queryByRole("textbox")).not.toBeInTheDocument();
    }
    // This device made the page: it is named, and there is no editor.
    const made = within(propertyRow(panel, "Made by")).getByLabelText(/^Made by: /u);
    expect(made.textContent).not.toBe(t("properties.computed.unknownDevice"));
    expect(within(propertyRow(panel, "Made by")).queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("are not offered by 'Add a property' and cannot be hidden", async () => {
    const { user } = await renderProperties([SPACE]);
    const panel = await openProperties(user, "Plain");
    await user.click(within(panel).getByRole("button", { name: t("properties.add") }));
    expect(screen_menu_names()).not.toContain("Created");
    expect(screen_menu_names()).toContain("Where");
  });
});

function screen_menu_names(): string[] {
  return Array.from(document.querySelectorAll(".properties-add-name")).map((node) => node.textContent ?? "");
}
