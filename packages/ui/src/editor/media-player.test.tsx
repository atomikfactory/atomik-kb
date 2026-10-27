// The player of a video or audio file block (ADR-0032): the pure rules, the player over the shell's
// streaming URL and over a blob, the card and the toggle, errors, read-only editors, the lock, and
// `/video` and `/audio`. Whole-app tests over the in-memory core.

import type { JsonValue, MemoryTransportOptions } from "@atomik-kb/core-client";
import { MAX_PREVIEW_BYTES } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { t } from "../i18n";
import { MIB, stubObjectUrls } from "../test/attachments";
import { pid } from "../test/editor";
import { openPage, renderPages } from "../test/page";
import { PASSPHRASE } from "../test/security";
import { sidebarButton } from "../test/sidebar";
import { mediaKindOf, parseFileView } from "./attachment-nodes";
import { AUDIO_ACCEPT, VIDEO_ACCEPT } from "./attachments";
import { describeElementError } from "./MediaPlayer";
import { pdfStopsEvent } from "./PdfViewer";

const SOUND = "a0000000-0000-4000-8000-0000000000a1";
const MOVIE = "a0000000-0000-4000-8000-0000000000a2";
const BIGMOVIE = "a0000000-0000-4000-8000-0000000000a3";
const TEXT = "a0000000-0000-4000-8000-0000000000a4";

function seed(): MemoryTransportOptions["attachments"] {
  return [
    { attachmentId: SOUND, name: "tone.wav", mime: "audio/wav", bytes: new Uint8Array([1, 2, 3, 4]) },
    { attachmentId: MOVIE, name: "clip.mp4", mime: "video/mp4", bytes: new Uint8Array([5, 6, 7]) },
    { attachmentId: BIGMOVIE, name: "film.mp4", mime: "video/mp4", bytes: new Uint8Array(MAX_PREVIEW_BYTES + MIB) },
    { attachmentId: TEXT, name: "notes.txt", mime: "text/plain", bytes: new TextEncoder().encode("hello") },
  ];
}

let urls: ReturnType<typeof stubObjectUrls>;
let pause: ReturnType<typeof vi.fn>;
let load: ReturnType<typeof vi.fn>;

beforeEach(() => {
  urls = stubObjectUrls();
  // jsdom does not play media: these only record that the element was stopped.
  pause = vi.fn();
  load = vi.fn();
  HTMLMediaElement.prototype.pause = pause as () => void;
  HTMLMediaElement.prototype.load = load as () => void;
});

afterEach(() => {
  vi.restoreAllMocks();
});

function pageWith(attachmentId: string, view?: string): JsonValue {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Listen:" }] },
      { type: "file", attrs: { id: pid(1), attachmentId, ...(view === undefined ? {} : { view }) } },
    ],
  };
}

const space = (content: JsonValue) => [{ name: "Personal", pages: [{ title: "Notes", content }] }];

/** Streams from the shell's protocol URL (the desktop), like `TauriTransport.attachments.mediaUrl`. */
const mediaUrl = (id: string): string => `http://kbmedia.localhost/${id}`;

async function openFile(attachmentId: string, view?: string, { stream = false }: { stream?: boolean } = {}) {
  const app = await renderPages(space(pageWith(attachmentId, view)), { attachments: seed(), ...(stream ? { mediaUrl } : {}) });
  const editor = await openPage(app.user, "Notes");
  return { ...app, editor };
}

function fileAttrs(editor: Editor): Record<string, unknown> {
  return editor.state.doc.child(1).attrs;
}

function player(editor: Editor, selector = "audio, video"): HTMLMediaElement {
  const element = editor.view.dom.querySelector(selector);
  if (!(element instanceof HTMLMediaElement)) {
    throw new Error("no player");
  }
  return element;
}

describe("the rules", () => {
  it("decides the player by the MIME type alone", () => {
    expect(mediaKindOf("video/mp4")).toBe("video");
    expect(mediaKindOf(" Audio/MPEG ")).toBe("audio");
    for (const mime of ["", "audio/", "video", "application/ogg", "application/octet-stream", "image/png", "application/pdf"]) {
      expect(mediaKindOf(mime), mime).toBeNull();
    }
    expect(parseFileView("preview")).toBe("preview");
  });

  it("leaves the events of the player to the player", () => {
    const inside = document.createElement("div");
    inside.className = "media-preview";
    const child = document.createElement("audio");
    inside.append(child);
    const event = new Event("keydown", { bubbles: true });
    Object.defineProperty(event, "target", { value: child });
    expect(pdfStopsEvent(event)).toBe(true);
  });

  it("names the formats the choosers list and the lines for a failing element", () => {
    expect(VIDEO_ACCEPT.split(",")).toEqual(expect.arrayContaining(["video/*", ".mp4", ".webm"]));
    expect(AUDIO_ACCEPT.split(",")).toEqual(expect.arrayContaining(["audio/*", ".mp3", ".wav", ".ogg"]));
    expect(describeElementError(4)).toBe(t("media.error.unsupported"));
    expect(describeElementError(3)).toBe(t("media.error.failed"));
    expect(describeElementError(undefined)).toBe(t("media.error.failed"));
  });
});

describe("the player", () => {
  it("plays an audio file from the streaming URL, with native controls, making no blob", async () => {
    const { editor } = await openFile(SOUND, "preview", { stream: true });
    await waitFor(() => {
      expect(editor.view.dom.querySelector("audio")).not.toBeNull();
    });
    const audio = player(editor, "audio");
    await waitFor(() => {
      expect(audio).toHaveAttribute("src", `http://kbmedia.localhost/${SOUND}`);
    });
    expect(audio.controls).toBe(true);
    expect(audio).toHaveAttribute("preload", "metadata");
    expect(audio).toHaveAccessibleName(t("media.label", { name: "tone.wav" }));
    expect(within(editor.view.dom).getByText("tone.wav")).toBeInTheDocument();
    expect(urls.create).not.toHaveBeenCalled();
  });

  it("plays a video the same way", async () => {
    const { editor } = await openFile(MOVIE, "preview", { stream: true });
    await waitFor(() => {
      expect(editor.view.dom.querySelector("video")).not.toBeNull();
    });
    expect(player(editor, "video")).toHaveAttribute("src", `http://kbmedia.localhost/${MOVIE}`);
    expect(player(editor, "video")).toHaveAttribute("preload", "metadata");
  });

  it("falls back to a blob of the file where there is no streaming URL, and revokes it on removal", async () => {
    const { editor, user } = await openFile(SOUND, "preview");
    await waitFor(() => {
      expect(player(editor, "audio")).toHaveAttribute("src", "blob:test/1");
    });
    expect(urls.create.mock.calls[0]?.[0]).toMatchObject({ size: 4, type: "audio/wav" });
    await user.click(within(editor.view.dom).getByRole("button", { name: t("attachment.actions") }));
    await user.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: t("attachment.remove") }));
    await waitFor(() => {
      expect(urls.revoke).toHaveBeenCalledWith("blob:test/1");
    });
  });

  it("shows the card with a message for a file too large for a blob", async () => {
    const { editor } = await openFile(BIGMOVIE, "preview");
    await waitFor(() => {
      expect(within(editor.view.dom).getByText(t("media.error.tooLarge"))).toBeInTheDocument();
    });
    expect(editor.view.dom.querySelector("video")).toBeNull();
    expect(fileAttrs(editor).view).toBe("preview");
  });

  it("streams a file beyond MAX_PREVIEW_BYTES", async () => {
    const { editor } = await openFile(BIGMOVIE, "preview", { stream: true });
    await waitFor(() => {
      expect(player(editor, "video")).toHaveAttribute("src", `http://kbmedia.localhost/${BIGMOVIE}`);
    });
  });

  it("goes back to the card with a message when the webview cannot decode the file", async () => {
    const { editor } = await openFile(SOUND, "preview", { stream: true });
    const audio = player(editor, "audio");
    await waitFor(() => {
      expect(audio).toHaveAttribute("src");
    });
    Object.defineProperty(audio, "error", { configurable: true, value: { code: 4 } });
    fireEvent.error(audio);
    await waitFor(() => {
      expect(within(editor.view.dom).getByText(t("media.error.unsupported"))).toBeInTheDocument();
    });
    expect(editor.view.dom.querySelector("audio")).toBeNull();
    expect(editor.view.dom.querySelector(".file-card.is-error")).not.toBeNull();
    expect(fileAttrs(editor).view).toBe("preview");
  });

  it("is a card again with 'Show as card' and a player again with 'Show player'", async () => {
    const { editor, user } = await openFile(SOUND, "preview", { stream: true });
    const menu = async () => {
      await user.click(within(editor.view.dom).getByRole("button", { name: t("attachment.actions") }));
      return screen.findByRole("menu");
    };
    await waitFor(() => {
      expect(editor.view.dom.querySelector("audio")).not.toBeNull();
    });
    await user.click(within(await menu()).getByRole("menuitem", { name: t("attachment.pdf.showCard") }));
    expect(fileAttrs(editor).view).toBeNull();
    expect(editor.view.dom.querySelector("audio")).toBeNull();
    expect(editor.view.dom.querySelector(".file-card")).not.toBeNull();
    await user.click(within(await menu()).getByRole("menuitem", { name: t("attachment.media.showPlayer") }));
    expect(fileAttrs(editor).view).toBe("preview");
    await waitFor(() => {
      expect(editor.view.dom.querySelector("audio")).not.toBeNull();
    });
  });

  it("keeps showing the card, with a toggle, for a media file block without a view", async () => {
    const { editor, user } = await openFile(MOVIE, undefined, { stream: true });
    expect(editor.view.dom.querySelector("video")).toBeNull();
    await user.click(within(editor.view.dom).getByRole("button", { name: t("attachment.actions") }));
    expect(within(await screen.findByRole("menu")).getByRole("menuitem", { name: t("attachment.media.showPlayer") })).toBeInTheDocument();
  });

  it("offers no player and no toggle for a file that is not media, even with a view", async () => {
    const { editor, user } = await openFile(TEXT, "preview", { stream: true });
    expect(editor.view.dom.querySelector("audio, video")).toBeNull();
    await user.click(within(editor.view.dom).getByRole("button", { name: t("attachment.actions") }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).queryByRole("menuitem", { name: t("attachment.media.showPlayer") })).toBeNull();
    expect(within(menu).queryByRole("menuitem", { name: t("attachment.pdf.showCard") })).toBeNull();
  });

  it("plays in a read-only editor, without the toggle or the remove entry", async () => {
    const { editor, user } = await openFile(SOUND, "preview", { stream: true });
    act(() => {
      editor.setEditable(false);
    });
    await waitFor(() => {
      expect(editor.view.dom.querySelector("audio")).not.toBeNull();
    });
    await user.click(within(editor.view.dom).getByRole("button", { name: t("attachment.actions") }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).queryByRole("menuitem", { name: t("attachment.pdf.showCard") })).toBeNull();
    expect(within(menu).queryByRole("menuitem", { name: t("attachment.remove") })).toBeNull();
    expect(within(menu).getByRole("menuitem", { name: t("attachment.copyMarkdown") })).toBeInTheDocument();
  });
});

describe("the lock", () => {
  it("stops the player and drops its source when the app locks", async () => {
    const app = await renderPages(space(pageWith(SOUND, "preview")), {
      attachments: seed(),
      mediaUrl,
      security: { kitCreatedAt: Date.UTC(2026, 9, 3, 12), appLockPassphrase: PASSPHRASE },
    });
    const editor = await openPage(app.user, "Notes");
    const audio = player(editor, "audio");
    await waitFor(() => {
      expect(audio).toHaveAttribute("src", `http://kbmedia.localhost/${SOUND}`);
    });
    pause.mockClear();
    load.mockClear();
    await app.user.click(sidebarButton(t("sidebar.lockNow")));
    await screen.findByLabelText(t("lock.passphraseLabel"));
    expect(audio).not.toHaveAttribute("src");
    expect(pause).toHaveBeenCalled();
    expect(load).toHaveBeenCalled();
    expect(document.querySelector("audio, video")).toBeNull();
  });
});

describe("/video and /audio", () => {
  function chooser(files: File[] | "cancel") {
    const inputs: HTMLInputElement[] = [];
    vi.spyOn(HTMLInputElement.prototype, "click").mockImplementation(function click(this: HTMLInputElement) {
      if (this.type !== "file") {
        return;
      }
      inputs.push(this);
      setTimeout(() => {
        if (files === "cancel") {
          this.dispatchEvent(new Event("cancel"));
        } else {
          Object.defineProperty(this, "files", { configurable: true, value: files });
          this.dispatchEvent(new Event("change"));
        }
      }, 0);
    });
    return inputs;
  }

  async function runSlash(command: string, title: string) {
    const app = await renderPages(space({ type: "doc", content: [{ type: "paragraph", attrs: { id: pid(0) } }] }), { mediaUrl });
    const editor = await openPage(app.user, "Notes");
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 1)));
    editor.commands.insertContent(command);
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(within(menu).getAllByRole("option")[0]?.querySelector(".suggestion-title")?.textContent).toBe(title);
    });
    return { ...app, editor, menu };
  }

  it("are found by name and by their keywords", async () => {
    const { filterSlashCommands } = await import("./slash-commands");
    expect(filterSlashCommands("video")[0]?.id).toBe("video");
    expect(filterSlashCommands("audio")[0]?.id).toBe("audio");
    for (const word of ["movie", "mp4"]) {
      expect(filterSlashCommands(word).map((command) => command.id), word).toContain("video");
    }
    for (const word of ["music", "podcast", "mp3"]) {
      expect(filterSlashCommands(word).map((command) => command.id), word).toContain("audio");
    }
  });

  it("/audio asks for a sound with the chooser, uploads it and inserts a block that plays", async () => {
    const wav = new File([new Uint8Array([9, 9, 9])], "voice.wav", { type: "audio/wav" });
    const inputs = chooser([wav]);
    const { editor, user, menu } = await runSlash("/audio", "Audio");
    await user.click(within(menu).getByText("Audio"));
    await waitFor(() => {
      expect(editor.state.doc.firstChild?.type.name).toBe("file");
    });
    expect(inputs[0]?.accept).toBe(AUDIO_ACCEPT);
    expect(inputs[0]?.multiple).toBe(false);
    expect(editor.state.doc.firstChild?.attrs.view).toBe("preview");
    expect(editor.state.doc.textContent).not.toContain("/audio");
    await waitFor(() => {
      expect(editor.view.dom.querySelector("audio")).not.toBeNull();
    });
  });

  it("/video inserts a block that plays a video", async () => {
    const mp4 = new File([new Uint8Array([1, 2])], "clip.mp4", { type: "video/mp4" });
    const inputs = chooser([mp4]);
    const { editor, user, menu } = await runSlash("/video", "Video");
    await user.click(within(menu).getByText("Video"));
    await waitFor(() => {
      expect(editor.view.dom.querySelector("video")).not.toBeNull();
    });
    expect(inputs[0]?.accept).toBe(VIDEO_ACCEPT);
  });

  it("makes the usual card of a file that is not media", async () => {
    chooser([new File(["x"], "notes.txt", { type: "text/plain" })]);
    const picked = await runSlash("/audio", "Audio");
    await picked.user.click(within(picked.menu).getByText("Audio"));
    await waitFor(() => {
      expect(picked.editor.state.doc.firstChild?.type.name).toBe("file");
    });
    expect(picked.editor.state.doc.firstChild?.attrs.view ?? null).toBeNull();
  });

  it("leaves nothing behind when the chooser is cancelled", async () => {
    chooser("cancel");
    const { editor, user, menu } = await runSlash("/audio", "Audio");
    await user.click(within(menu).getByText("Audio"));
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20));
    });
    expect(editor.state.doc.childCount).toBe(1);
    expect(editor.state.doc.firstChild?.type.name).toBe("paragraph");
  });
});
