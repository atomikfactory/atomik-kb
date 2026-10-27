/**
 * Version of the request/response/event contract this client was built for. Must equal
 * `kb_types::API_VERSION`; the UI compares it with `AppStatus.apiVersion` and warns on
 * a mismatch. A breaking change in `kb-types` bumps both.
 */
export const API_VERSION = 1;
