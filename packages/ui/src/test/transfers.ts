// Attachment sync (Milestone 8) for tests: the memory core holds every chunk of its
// attachments, so this wrapper makes chosen attachments report another availability
// (downloading, waiting, not downloaded) or none at all (a manifest that has not arrived),
// and delivers `AttachmentsChanged` as the core does when a chunk arrives.

import { CoreError } from "@atomik-kb/core-client";
import type { AttachmentId, AttachmentInfo, Event as CoreEvent, Request, Response, SpaceId, Transport } from "@atomik-kb/core-client";
import { act } from "@testing-library/react";

/** What a test says about one attachment: fields to report instead of the core's, or `absent`. */
export type TransferState = Partial<AttachmentInfo> | "absent";

/**
 * Rewrites the `Attachment` and `Attachments` answers of `transport` (a plain object, changed in
 * place) for the attachments given a state, answers `NotFound` for the `absent` ones, and moves
 * an attachment asked for with `AttachmentDownload` to `Downloading` (none of its chunks yet).
 */
export function withTransfers(transport: Transport) {
  const states = new Map<AttachmentId, TransferState>();
  const listeners = new Set<(event: CoreEvent) => void>();
  const dispatch = transport.dispatch.bind(transport);
  const subscribe = transport.subscribe.bind(transport);

  const patch = (info: AttachmentInfo): AttachmentInfo => {
    const state = states.get(info.attachmentId);
    return state === undefined || state === "absent" ? info : { ...info, ...state };
  };

  const absent = (request: Request): boolean =>
    (request.type === "AttachmentGet" || request.type === "AttachmentDownload") &&
    states.get(request.data.attachmentId) === "absent";

  transport.dispatch = async (request: Request): Promise<Response> => {
    if (absent(request)) {
      return Promise.reject(new CoreError({ code: "NotFound", message: "attachment not found" }));
    }
    if (request.type === "AttachmentDownload") {
      const state = states.get(request.data.attachmentId);
      if (typeof state === "object" && state.availability !== "Complete") {
        states.set(request.data.attachmentId, { ...state, availability: "Downloading" });
      }
    }
    const response = await dispatch(request);
    if (response.type === "Attachment") {
      return { ...response, data: patch(response.data) };
    }
    if (response.type === "Attachments") {
      return { ...response, data: response.data.filter((info) => states.get(info.attachmentId) !== "absent").map(patch) };
    }
    return response;
  };

  transport.subscribe = (onEvent) => {
    listeners.add(onEvent);
    const unsubscribe = subscribe(onEvent);
    return () => {
      listeners.delete(onEvent);
      unsubscribe();
    };
  };

  return {
    /** Sets what the attachment reports from now on; `null` gives it back to the core. */
    set(attachmentId: AttachmentId, state: TransferState | null): void {
      if (state === null) {
        states.delete(attachmentId);
      } else {
        states.set(attachmentId, state);
      }
    },
    /** Delivers `AttachmentsChanged { spaceId }` to the app, as the core does when a chunk arrives. */
    changed(spaceId: SpaceId): void {
      act(() => {
        for (const listener of listeners) {
          listener({ type: "AttachmentsChanged", data: { spaceId } });
        }
      });
    },
  };
}
