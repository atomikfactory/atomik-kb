import type { ApiError } from "./generated/ApiError";
import type { ErrorCode } from "./generated/ErrorCode";
import type { JsonValue } from "./generated/serde_json/JsonValue";

/**
 * Every `ErrorCode` the contract knows. Typed as a `Record` so adding a code to
 * `kb-types` (and regenerating) fails the type check here until it is listed.
 */
const ERROR_CODES: Record<ErrorCode, true> = {
  NotFound: true,
  InvalidInput: true,
  AlreadyExists: true,
  Storage: true,
  Crypto: true,
  KeystoreUnavailable: true,
  Locked: true,
  Internal: true,
  Forbidden: true,
};

function isErrorCode(value: unknown): value is ErrorCode {
  return typeof value === "string" && Object.hasOwn(ERROR_CODES, value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

/**
 * Type guard for the serialized `ApiError` (`{ code, message, details? }`) that the
 * core, the Tauri shell and the transports hand to the UI. Accepts plain objects as
 * well as {@link CoreError} instances.
 */
export function isApiError(value: unknown): value is ApiError {
  return isRecord(value) && isErrorCode(value.code) && typeof value.message === "string";
}

/**
 * An `Error` that is also an {@link ApiError}: carries `code`, `message` and `details`
 * and a stack trace. Everything this package throws or rejects with is a `CoreError`,
 * so callers can use `isApiError` or `instanceof CoreError` interchangeably.
 */
export class CoreError extends Error implements ApiError {
  readonly code: ErrorCode;
  readonly details?: JsonValue;

  constructor(error: ApiError, options?: ErrorOptions) {
    super(error.message, options);
    this.name = "CoreError";
    this.code = error.code;
    if (error.details !== undefined) {
      this.details = error.details;
    }
  }

  /** The plain, serializable `ApiError` (what the core produced). */
  toApiError(): ApiError {
    const plain: ApiError = { code: this.code, message: this.message };
    if (this.details !== undefined) {
      plain.details = this.details;
    }
    return plain;
  }

  toJSON(): ApiError {
    return this.toApiError();
  }
}

/** Shorthand for `new CoreError({ code: "Internal", message })`. */
export function internalError(message: string, options?: ErrorOptions): CoreError {
  return new CoreError({ code: "Internal", message }, options);
}

/**
 * Normalises anything a transport can fail with into a {@link CoreError}.
 *
 * - an `ApiError`-shaped object (what Tauri's `invoke` rejects with when the core
 *   returned `Err(ApiError)`) keeps its code, message and details;
 * - an object that looks like an `ApiError` but carries a code this build does not
 *   know (a newer core) becomes `Internal`, keeping the original code in the message;
 * - an `Error` or a string (Tauri reports command and argument problems as strings)
 *   becomes `Internal` with that text;
 * - anything else becomes `Internal` with a generic message.
 */
export function toCoreError(error: unknown): CoreError {
  if (error instanceof CoreError) {
    return error;
  }
  if (isApiError(error)) {
    return new CoreError(error);
  }
  if (isRecord(error) && typeof error.code === "string" && typeof error.message === "string") {
    return internalError(`${error.code}: ${error.message}`);
  }
  if (error instanceof Error) {
    return internalError(error.message, { cause: error });
  }
  if (typeof error === "string" && error.length > 0) {
    return internalError(error);
  }
  return internalError("Unknown error");
}
