// Helpers for the tests of shared spaces (Milestone 13): the app over the in-memory core with a Work space that starts shared
// (or as a read-only copy) for a role, the simulated people of the mirror, and the queries the tests share.

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type {
  ArchiveReason,
  MemberRole,
  MemorySharingOptions,
  MemoryTransportOptions,
  SeedSpace,
  Transport,
} from "@atomik-kb/core-client";
import { screen, waitFor, within } from "@testing-library/react";
import { expect } from "vitest";

import { t } from "../i18n";
import { paragraphs } from "./editor";
import { NOW, findMainNavigation, renderWith } from "./render";
import type { User } from "./render";
import { row } from "./sidebar";

export const SERVER = "https://sync.example.com";

/** The spaces of the tests: Personal (private) and Work (the one that is shared). */
export const SHARING_SPACES: readonly SeedSpace[] = [
  { name: "Personal", pages: [{ title: "Diary", content: paragraphs("Dear diary") }] },
  {
    name: "Work",
    pages: [
      { title: "Plan", content: paragraphs("Ship it"), children: [{ title: "Sub plan" }] },
      { title: "Notes", content: paragraphs("Call the dentist") },
    ],
  },
];

export interface SharingRender {
  /** `"personal"` leaves Work personal; a role shares it; default `owner`. */
  role?: MemberRole | "personal";
  /** Starts Work as a read-only copy. */
  archive?: ArchiveReason;
  /** More of `MemorySharingOptions` (merged over the seed of Work). */
  sharing?: MemorySharingOptions;
  /** More options of the transport. */
  memory?: MemoryTransportOptions;
  /** Wraps the transport (a shell, a recorder). */
  wrap?: (memory: MemoryTransport) => Transport;
}

/**
 * The app over a workspace whose Work space is shared for `role`, with a kit, sync on and the profile name "Maya", and waits for
 * the sidebar. The sessions of the mirror take 30 ms a step.
 */
export async function renderSharing(options: SharingRender = {}) {
  const role = options.role ?? "owner";
  const memory = new MemoryTransport({
    now: () => NOW,
    workspaceName: "Research",
    spaces: SHARING_SPACES,
    security: { kitCreatedAt: NOW },
    sync: { state: "idle", serverUrl: SERVER },
    syncStepMs: 5,
    pairingStepMs: 30,
    sharing: {
      profileName: "Maya",
      stepMs: 30,
      ...(role === "personal"
        ? {}
        : { spaces: [{ space: "Work", role, ...(options.archive === undefined ? {} : { archive: options.archive }) }] }),
      ...options.sharing,
    },
    ...options.memory,
  });
  const app = renderWith(options.wrap?.(memory) ?? memory);
  await findMainNavigation();
  return { ...app, memory, client: new CoreClient(memory) };
}

/** Switches to a space by its name through the space list. */
export async function openSpace(user: User, name: string): Promise<void> {
  const button = screen.getByRole("button", { description: t("spaces.switchTitle") });
  await user.click(button);
  const list = await screen.findByRole("list", { name: t("spaces.listLabel") });
  await user.click(within(list).getByRole("button", { name: new RegExp(`^${name}`, "u") }));
  await waitFor(() => {
    expect(screen.getByRole("button", { description: t("spaces.switchTitle") })).toHaveTextContent(name);
  });
}

/** Opens the space menu (the "..." beside the space name) and chooses an item. */
export async function spaceMenu(user: User, label: string): Promise<void> {
  await user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
  const menu = await screen.findByRole("menu", { name: t("spaces.optionsLabel") });
  await user.click(within(menu).getByRole("menuitem", { name: label }));
}

/** The items the space menu offers, in order. */
export async function spaceMenuItems(user: User): Promise<string[]> {
  await user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
  const menu = await screen.findByRole("menu", { name: t("spaces.optionsLabel") });
  const items = within(menu)
    .getAllByRole("menuitem")
    .map((item) => item.textContent ?? "");
  await user.keyboard("{Escape}");
  return items;
}

/** Opens "Members and sharing..." of the current space and answers the dialog. */
export async function openMembers(user: User): Promise<HTMLElement> {
  await spaceMenu(user, t("sharing.menu.members"));
  const dialog = await screen.findByRole("dialog", { name: t("sharing.members.title") });
  // The list is there once the members were read.
  await within(dialog).findByRole("list", { name: t("sharing.members.list") });
  return dialog;
}

/** One member row of the panel, found by name. */
export function memberRow(dialog: HTMLElement, name: string): HTMLElement {
  const list = within(dialog).getByRole("list", { name: t("sharing.members.list") });
  const found = within(list)
    .getAllByRole("listitem")
    .find((item) => item.querySelector(".member-name")?.textContent === name);
  if (found === undefined) {
    throw new Error(`no member row for ${name}`);
  }
  return found;
}

/** The names of the members, in the order the panel lists them. */
export function memberNames(dialog: HTMLElement): string[] {
  const list = within(dialog).getByRole("list", { name: t("sharing.members.list") });
  return within(list)
    .getAllByRole("listitem")
    .map((item) => item.querySelector(".member-name")?.textContent ?? "");
}

/** Opens a page of the sidebar tree of the current space (without waiting for an editor). */
export async function clickPage(user: User, title: string): Promise<void> {
  await user.click(row(title));
  await screen.findByRole("heading", { level: 1, name: title });
}

/** The toasts' region. */
export const toasts = (): HTMLElement => screen.getByRole("log", { name: t("toast.label") });
