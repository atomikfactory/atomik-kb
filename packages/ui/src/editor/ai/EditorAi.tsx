// The AI features of one page editor (ADR-0037, `docs/architecture/ai.md` section 8): the writing panel's
// controller, the recorder and the transcription jobs, created once per editor and handed down through
// React context (the node views and menus below read it) and a registry by editor (the `/` menu, which
// runs outside React, reads it). `AiEditorProvider` wraps the editor's views; it also draws the panel and
// the recorder bar, and registers the editor as the front page's (for the chat panel's "Insert into page"; the editor of a
// peek, ADR-0054, is not the front page's and never registers, so the page in front keeps the slot whatever the peek does).

import type { Editor } from "@tiptap/core";
import { createContext, useContext, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";

import { spaceAiOn } from "../../sharing";
import { useAppStore, useStoreApi } from "../../store-context";
import { AI_SETTINGS_SETTING } from "@atomik-kb/core-client";
import { referencedTitle } from "../node-views";
import { serviceOf } from "../attachments";
import { dictationRunning } from "../dictation/registry";
import { clearActiveEditor, setActiveEditor } from "./active-editor";
import { useAiClient } from "./ai-client";
import type { AiEditorClient } from "./ai-client";
import { AiWriterPanel } from "./AiWriterPanel";
import { NO_AI, useAiCapabilities } from "./availability";
import type { AiCapabilities } from "./availability";
import { RecorderBar } from "./RecorderBar";
import { RecorderController } from "./recorder";
import { TranscribeJobs } from "./transcribe-jobs";
import { WriterController } from "./writer";

/** The AI features of one editor. */
export interface EditorAi {
  writer: WriterController;
  recorder: RecorderController;
  jobs: TranscribeJobs;
  /** What is on offer now (the latest settings). */
  capabilities: () => AiCapabilities;
}

/** A value the controllers read when they act, set by the provider as the app changes. */
class Latest<T> {
  #value: T;

  constructor(initial: T) {
    this.#value = initial;
  }

  readonly get = (): T => this.#value;

  set(value: T): void {
    this.#value = value;
  }
}

interface Env {
  client: AiEditorClient | null;
  capabilities: AiCapabilities;
}

const registry = new WeakMap<Editor, EditorAi>();

/** The AI features of an editor, `null` for an editor without them (a schema-only editor, a test). */
export function editorAiOf(editor: Editor): EditorAi | null {
  return registry.get(editor) ?? null;
}

/** Whether the writing assistant is on offer in `editor` (the `/` menu hides its entries when it is not). */
export function writingOn(editor: Editor): boolean {
  return registry.get(editor)?.capabilities().writing === true;
}

/** Whether the recorder is on offer in `editor`. */
export function recordingOn(editor: Editor): boolean {
  return registry.get(editor)?.capabilities().enabled === true;
}

const EditorAiContext = createContext<EditorAi | null>(null);

/** The AI features of the editor the component is in (`null` outside the page editor). */
export function useEditorAi(): EditorAi | null {
  return useContext(EditorAiContext);
}

interface ProviderProps {
  editor: Editor | null;
  pageId: string;
  /** The page's space: a space whose AI is off (a shared one until this account opts in, Milestone 13) offers no AI feature. */
  spaceId?: string;
  /** The editor is the page in front's and registers for the chat panel (the default); the peek's editor passes `false`. */
  front?: boolean;
  children: ReactNode;
}

export function AiEditorProvider({ editor, pageId, spaceId, front = true, children }: ProviderProps) {
  const store = useStoreApi();
  const client = useAiClient();
  const settingsCapabilities = useAiCapabilities();
  const spaceAi = useAppStore((state) => spaceAiOn(state.spaces.find((space) => space.id === spaceId)));
  const capabilities = spaceAi ? settingsCapabilities : NO_AI;
  const [latest] = useState(() => new Latest<Env>({ client: null, capabilities: NO_AI }));
  const loaded = useAppStore((state) => AI_SETTINGS_SETTING in state.settings);

  useEffect(() => {
    latest.set({ client, capabilities });
  }, [latest, client, capabilities]);

  // The settings are read once, so that the entries show without a visit to Settings.
  useEffect(() => {
    if (client?.hasAi() === true && !loaded) {
      store
        .getState()
        .loadSetting(AI_SETTINGS_SETTING)
        .catch(() => undefined);
    }
  }, [client, loaded, store]);

  const ai = useMemo<EditorAi | null>(() => {
    if (editor === null) {
      return null;
    }
    const resolvers = {
      titleOf: (id: string) => referencedTitle(store.getState(), id),
      nameOf: (id: string) => {
        const lookup = store.getState().attachmentLookup[id];
        return lookup?.status === "found" ? lookup.info.name : null;
      },
    };
    const clientNow = (): AiEditorClient | null => latest.get().client;
    const capabilitiesNow = (): AiCapabilities => latest.get().capabilities;
    return {
      writer: new WriterController({
        editor,
        client: clientNow,
        title: () => referencedTitle(store.getState(), pageId) ?? "",
        resolvers,
      }),
      recorder: new RecorderController({
        editor,
        client: clientNow,
        capabilities: capabilitiesNow,
        upload: () => {
          const service = serviceOf(editor);
          return service?.canUpload === true ? service.upload : null;
        },
        // Dictation and the recorder both want the microphone and the cursor (ADR-0053): neither starts over the other.
        blocked: () => dictationRunning(editor),
      }),
      jobs: new TranscribeJobs({
        editor,
        client: clientNow,
        capabilities: capabilitiesNow,
        readRange: (attachmentId, range) => store.getState().readAttachmentRange(attachmentId, range),
      }),
      capabilities: capabilitiesNow,
    };
  }, [editor, latest, store, pageId]);

  useEffect(() => {
    if (ai === null || editor === null) {
      return;
    }
    registry.set(editor, ai);
    if (front) {
      setActiveEditor({ pageId, editor });
    }
    const detach = [ai.writer.attach(), ai.recorder.attach(), ai.jobs.attach()];
    return () => {
      for (const end of detach) {
        end();
      }
      if (front) {
        clearActiveEditor(editor);
      }
      if (registry.get(editor) === ai) {
        registry.delete(editor);
      }
    };
  }, [ai, editor, pageId, front]);

  return (
    <EditorAiContext value={ai}>
      {children}
      {ai !== null && <AiWriterPanel controller={ai.writer} />}
      {ai !== null && <RecorderBar controller={ai.recorder} />}
    </EditorAiContext>
  );
}
