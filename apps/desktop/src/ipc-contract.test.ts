import { describe, expect, it } from "vitest";

import presenceStateRaw from "../../../packages/core-client/src/generated/PresenceState.ts?raw";
import dictationTauriRaw from "../../../packages/core-client/src/dictation-tauri.ts?raw";
import driveRaw from "../../../packages/core-client/src/drive.ts?raw";
import driveTauriRaw from "../../../packages/core-client/src/drive-tauri.ts?raw";
import transportRaw from "../../../packages/core-client/src/tauri.ts?raw";
import transportTypesRaw from "../../../packages/core-client/src/transport.ts?raw";
import aiHostRaw from "../src-tauri/src/ai_host.rs?raw";
import attachmentIpcRaw from "../src-tauri/src/attachment_ipc.rs?raw";
import attachmentsRaw from "../src-tauri/src/attachments.rs?raw";
import backupRaw from "../src-tauri/src/backup.rs?raw";
import acceleratorRaw from "../src-tauri/src/accelerator.rs?raw";
import deepLinkRaw from "../src-tauri/src/deep_link.rs?raw";
import desktopSettingsRaw from "../src-tauri/src/desktop_settings.rs?raw";
import docIpcRaw from "../src-tauri/src/doc_ipc.rs?raw";
import driveHostRaw from "../src-tauri/src/drive_host.rs?raw";
import shellRaw from "../src-tauri/src/lib.rs?raw";
import menuBarRaw from "../src-tauri/src/menu_bar.rs?raw";
import shellEventsRaw from "../src-tauri/src/shell_events.rs?raw";
import shellLabelsRaw from "../src-tauri/src/shell_labels.rs?raw";
import shortcutRaw from "../src-tauri/src/shortcut.rs?raw";
import speechHostRaw from "../src-tauri/src/speech_host.rs?raw";
import stateRaw from "../src-tauri/src/state.rs?raw";
import enRaw from "../../../packages/ui/src/i18n/en.json?raw";
import uiAcceleratorRaw from "../../../packages/ui/src/accelerator.ts?raw";
import findBarRaw from "../../../packages/ui/src/editor/FindBar.tsx?raw";
import mainLayoutRaw from "../../../packages/ui/src/screens/MainLayout.tsx?raw";
import printRaw from "../../../packages/ui/src/print.ts?raw";
import shortcutsRaw from "../../../packages/ui/src/shortcuts.ts?raw";
import storeShellRaw from "../../../packages/ui/src/store-shell.ts?raw";

// The TypeScript transport and the Rust shell talk through command names, argument names and
// a header name that no compiler checks. These tests read both sources and fail when they drift apart.

/** The sources without their comments, so a doc comment that mentions a call cannot stand in for it. */
function code(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|\s)\/\/.*$/gm, "$1");
}

// The dictation commands (ADR-0053) and the Google Drive ones (ADR-0059) are invoked from their own modules, which
// `tauri.ts` re-exports.
const transportSource = `${code(transportRaw)}
${code(dictationTauriRaw)}
${code(driveTauriRaw)}`;
const transportTypesSource = code(transportTypesRaw);
const aiHostSource = code(aiHostRaw);
const speechHostSource = code(speechHostRaw);
const backupSource = code(backupRaw);
const docIpcSource = code(docIpcRaw);
const attachmentIpcSource = code(attachmentIpcRaw);
const attachmentsSource = code(attachmentsRaw);
const shellSource = code(shellRaw);
const stateSource = code(stateRaw);

/** The IPC contract: every command the shell registers and the transport invokes. */
const COMMANDS = [
  "core_state",
  "dispatch",
  "subscribe_events",
  "doc_open",
  "doc_import",
  "doc_export",
  "doc_close",
  "open_external",
  "lock",
  "unlock",
  "backup_export",
  "backup_import",
  "attachment_write",
  "attachment_read",
  "attachment_import_path",
  "attachment_import_url",
  "attachment_save_as",
  "attachment_open",
  "export_run",
  "import_run",
  "save_text_file",
  "backup_folder_choose",
  "backup_run_now",
  "ai_status",
  "ai_endpoint_set",
  "ai_generate",
  "ai_chat",
  "ai_cancel",
  "ai_search",
  "ai_transcribe",
  "mcp_status",
  "mcp_client_grants_set",
  "shell_events_subscribe",
  "shell_labels_set",
  "menu_bar_show",
  "menu_bar_hide",
  "presence_send",
  "presence_events",
  "dictation_status",
  "dictation_select",
  "dictation_install",
  "dictation_install_cancel",
  "dictation_import",
  "dictation_remove",
  "dictation_start",
  "dictation_audio",
  "dictation_stop",
  "dictation_cancel",
  "drive_status",
  "drive_connect",
  "drive_disconnect",
  "drive_client_set",
  "drive_backups_list",
  "drive_backup_download",
  "drive_cancel",
  "backup_folder_list",
];

/** The raw-bytes commands for page documents: the page id in a header, the payload as the body. */
const DOC_COMMANDS = ["doc_open", "doc_import", "doc_export", "doc_close"];

/** The raw-bytes commands for attachments: the attachment id in a header (and a byte range in two more), the plaintext as the body or the answer. */
const ATTACHMENT_RAW_COMMANDS = ["attachment_write", "attachment_read"];

/** Every raw-bytes command: `tauri::ipc::Request` in, `tauri::ipc::Response` out. */
const RAW_COMMANDS = [...DOC_COMMANDS, ...ATTACHMENT_RAW_COMMANDS];

/**
 * The raw request with a JSON answer (ADR-0037): `ai_transcribe` takes a WAV as its body and its options in a
 * header, and answers `{ text, language? }` as JSON. `dictation_audio` (ADR-0053) takes samples as its body and the
 * session in a header, and answers nothing.
 */
const RAW_REQUEST_COMMANDS = ["ai_transcribe", "dictation_audio"];

/** The commands whose arguments travel as one JSON object (everything but the raw-bytes ones). */
const JSON_COMMANDS = COMMANDS.filter((command) => !RAW_COMMANDS.includes(command) && !RAW_REQUEST_COMMANDS.includes(command));

/** `invoke(` with an optional generic argument (one level of nesting), up to the opening quote. */
const INVOKE = String.raw`invoke(?:<(?:[^<>()]|<[^<>()]*>)*>)?\(\s*`;

function registeredCommands(): string[] {
  const list = /generate_handler!\[([^\]]*)\]/.exec(shellSource)?.[1];
  expect(list, "tauri::generate_handler![...] not found in src-tauri/src/lib.rs").toBeDefined();
  return (list ?? "")
    .split(",")
    .map((name) => name.trim().split("::").pop() ?? "")
    .filter((name) => name !== "");
}

function invokedCommands(): string[] {
  return [...transportSource.matchAll(new RegExp(`${INVOKE}"([a-z_]+)"`, "g"))].map((match) => match[1] ?? "");
}

/**
 * The property names of the object literals the transport passes as the arguments of
 * `invoke("<command>", { ... })`; empty when the call passes none. Reads `{ a, b }` and `{ a: x, b: y }`
 * (no commas or braces inside a value). A call that leaves an optional argument out
 * (`path === undefined ? { spaceId } : { spaceId, path }`) passes several literals: the names are
 * those of all of them together, which is what the Rust command can receive.
 */
function invokedArgumentNames(command: string): string[] {
  const call = new RegExp(`${INVOKE}"${command}"`).exec(transportSource);
  expect(call, `invoke("${command}"...) not found in packages/core-client/src/tauri.ts`).not.toBeNull();
  // The text between the command name and the parenthesis that closes the call.
  const from = (call?.index ?? 0) + (call?.[0].length ?? 0);
  let depth = 1;
  let end = from;
  while (end < transportSource.length && depth > 0) {
    const character = transportSource[end];
    depth += character === "(" ? 1 : character === ")" ? -1 : 0;
    end += 1;
  }
  const names = new Set<string>();
  for (const literal of transportSource.slice(from, end - 1).matchAll(/\{([^{}]*)\}/g)) {
    for (const property of (literal[1] ?? "").split(",")) {
      const name = property.split(":")[0]?.trim() ?? "";
      if (name !== "") {
        names.add(name);
      }
    }
  }
  return [...names];
}

/** Splits at the commas that are not inside `<>`, `()` or `[]` (`State<'_, SharedState>` is one parameter). */
function splitTopLevel(text: string): string[] {
  const parts: string[] = [];
  let depth = 0;
  let current = "";
  for (const character of text) {
    if ("<([".includes(character)) {
      depth += 1;
    } else if (">)]".includes(character)) {
      depth -= 1;
    }
    if (character === "," && depth === 0) {
      parts.push(current);
      current = "";
    } else {
      current += character;
    }
  }
  parts.push(current);
  return parts.map((part) => part.trim()).filter((part) => part !== "");
}

/** The parameters of `async fn <command>(...)` in the shell, as `{ name, type }`. */
function rustParameters(command: string): { name: string; type: string }[] {
  const start = new RegExp(String.raw`async fn ${command}(?:<[^>]*>)?\(`).exec(shellSource);
  expect(start, `async fn ${command} not found in src-tauri/src/lib.rs`).not.toBeNull();
  const from = (start?.index ?? 0) + (start?.[0].length ?? 0);
  let depth = 1;
  let end = from;
  while (end < shellSource.length && depth > 0) {
    const character = shellSource[end];
    depth += character === "(" ? 1 : character === ")" ? -1 : 0;
    end += 1;
  }
  return splitTopLevel(shellSource.slice(from, end - 1)).map((parameter) => {
    const match = /^(\w+)\s*:\s*([\s\S]+)$/.exec(parameter);
    expect(match, `cannot read the parameter "${parameter}" of ${command}`).not.toBeNull();
    return { name: match?.[1] ?? "", type: match?.[2] ?? "" };
  });
}

/** Parameters Tauri fills in itself (the managed state, the app handle, the calling window); the webview never sends them. */
function isInjected(type: string): boolean {
  return /^(?:tauri::)?(?:State|AppHandle|Window|WebviewWindow|Webview)\b/.test(type);
}

/** `on_event` is `onEvent` on the wire: Tauri names the arguments of a command in camelCase. */
function camelCase(name: string): string {
  return name.replace(/_([a-z])/g, (_, letter: string) => letter.toUpperCase());
}

/** The variants of a Rust enum, in declaration order (a variant with fields in braces is one variant). */
function enumVariants(source: string, name: string): string[] {
  const start = new RegExp(String.raw`pub enum ${name}(?:<[^{]*>)? \{`).exec(source);
  expect(start, `enum ${name} not found`).not.toBeNull();
  const from = (start?.index ?? 0) + (start?.[0].length ?? 0);
  let depth = 1;
  let end = from;
  while (end < source.length && depth > 0) {
    const character = source[end];
    depth += character === "{" ? 1 : character === "}" ? -1 : 0;
    end += 1;
  }
  let body = source.slice(from, end - 1);
  // The fields of a variant are not variants: take the nested braces out, innermost first.
  for (let previous = ""; previous !== body; ) {
    previous = body;
    body = body.replace(/\{[^{}]*\}/g, "");
  }
  return [...body.matchAll(/^\s*([A-Z]\w*)/gm)].map((match) => match[1] ?? "");
}

describe("TauriTransport and the desktop shell", () => {
  it("the shell registers exactly the documented commands", () => {
    expect(registeredCommands().sort()).toEqual([...COMMANDS].sort());
  });

  it("the module documentation of the shell lists every command", () => {
    const table = shellRaw
      .split("\n")
      .filter((line) => line.startsWith("//! |"))
      .join("\n");
    for (const command of COMMANDS) {
      expect(table, command).toContain(`\`${command}\``);
    }
  });

  it("every command the transport invokes is registered", () => {
    const invoked = invokedCommands();
    expect(invoked.length).toBeGreaterThan(0);
    for (const command of invoked) {
      expect(registeredCommands(), command).toContain(command);
    }
  });

  it("the transport invokes every registered command", () => {
    for (const command of registeredCommands()) {
      expect(invokedCommands(), command).toContain(command);
    }
  });

  it("dispatch takes the request under the name the transport sends it", () => {
    expect(shellSource).toMatch(/async fn dispatch\([^)]*\brequest: Request\b/);
    expect(transportSource).toMatch(/invoke<Response>\("dispatch", \{ request \}\)/);
  });

  it("subscribe_events takes the channel under the name the transport sends it (onEvent is on_event in Rust)", () => {
    expect(shellSource).toMatch(/async fn subscribe_events\([^)]*\bon_event: Channel<Event>/);
    expect(transportSource).toMatch(/invoke\("subscribe_events", \{ onEvent: channel \}\)/);
  });

  it("core_state takes no arguments, as the transport calls it", () => {
    expect(shellSource).toMatch(/async fn core_state\(state: State<'_, SharedState>\)/);
    expect(transportSource).toMatch(/invoke<unknown>\("core_state"\)/);
  });

  it.each(RAW_COMMANDS)("%s is a raw IPC command: tauri::ipc::Request in, tauri::ipc::Response out", (command) => {
    const signature = new RegExp(
      String.raw`async fn ${command}\([^)]*\brequest: tauri::ipc::Request<'_>,?\s*\)\s*->\s*Result<tauri::ipc::Response, ApiError>`,
    );
    expect(shellSource).toMatch(signature);
  });

  it("both sides name the page header x-kb-page", () => {
    expect(docIpcSource).toContain('pub const PAGE_HEADER: &str = "x-kb-page";');
    expect(transportSource).toMatch(/["'`]x-kb-page["'`]/);
  });

  it("open_external takes the url under the name the transport sends it", () => {
    expect(shellSource).toMatch(/async fn open_external(?:<[^>]*>)?\([^)]*\burl: String\b/);
    expect(transportSource).toMatch(new RegExp(`${INVOKE}"open_external",\\s*\\{\\s*url(?:\\s*:\\s*\\w+)?\\s*\\}`));
  });

  // --- the app lock (ADR-0018) ---

  it("lock takes no arguments, as the transport calls it", () => {
    expect(shellSource).toMatch(/async fn lock\(state: State<'_, SharedState>\)\s*->\s*Result<\(\), ApiError>/);
    expect(transportSource).toMatch(new RegExp(`${INVOKE}"lock"\\s*\\)`));
  });

  it("unlock takes the passphrase under the name the transport sends it, zeroised from the start", () => {
    expect(shellSource).toMatch(/async fn unlock\([^)]*\bpassphrase: Zeroizing<String>/);
    expect(transportSource).toMatch(new RegExp(`${INVOKE}"unlock",\\s*\\{\\s*passphrase(?:\\s*:\\s*\\w+)?\\s*\\}`));
  });

  it("the transport understands every state core_state can report", () => {
    const variants = enumVariants(stateSource, "CoreStateInfo");
    // Rust serialises the variants in snake_case: "opening", "ready", "locked", { failed }.
    expect(variants.map((variant) => variant.toLowerCase())).toEqual(["opening", "ready", "locked", "failed"]);
    for (const variant of variants) {
      const name = variant.toLowerCase();
      expect(transportSource, `tauri.ts does not mention the core state "${name}"`).toMatch(
        new RegExp(`["'\`]${name}["'\`]`),
      );
      expect(transportTypesSource, `transport.ts does not mention the core state "${name}"`).toMatch(
        new RegExp(`["'\`]?${name}["'\`]?`),
      );
    }
  });

  // --- backups (ADR-0017) ---

  it("backup_export takes an optional path under the name the transport sends it", () => {
    expect(shellSource).toMatch(/async fn backup_export(?:<[^>]*>)?\([^)]*\bpath: Option<String>/);
    expect(transportSource).toMatch(new RegExp(`${INVOKE}"backup_export",\\s*\\{\\s*path(?:\\s*:\\s*[^,}]+)?\\s*\\}`));
  });

  it("backup_import takes an optional path and an optional kit (zeroised) under the names the transport sends", () => {
    expect(shellSource).toMatch(
      /async fn backup_import(?:<[^>]*>)?\([^)]*\bpath: Option<String>,\s*kit: Option<Zeroizing<String>>/,
    );
    expect(transportSource).toMatch(
      new RegExp(`${INVOKE}"backup_import",\\s*\\{\\s*path(?:\\s*:\\s*[^,}]+)?,\\s*kit(?:\\s*:\\s*[^,}]+)?\\s*\\}`),
    );
  });

  it("both backup commands answer { summary, path }, the shape of the transport's BackupResult", () => {
    const rustStruct = /pub struct BackupResult \{([^}]*)\}/.exec(backupSource)?.[1];
    expect(rustStruct, "BackupResult not found in src-tauri/src/backup.rs").toBeDefined();
    const rustFields = [...(rustStruct ?? "").matchAll(/pub (\w+):/g)].map((match) => camelCase(match[1] ?? ""));
    // The struct is serialised with `rename_all = "camelCase"`.
    expect(backupSource).toMatch(/#\[serde\(rename_all = "camelCase"\)\]\s*pub struct BackupResult/);

    const tsInterface = /export interface BackupResult \{([^}]*)\}/.exec(transportTypesSource)?.[1];
    expect(tsInterface, "BackupResult not found in packages/core-client/src/transport.ts").toBeDefined();
    const tsFields = [...(tsInterface ?? "").matchAll(/^\s*(\w+)\??\s*:/gm)].map((match) => match[1] ?? "");

    expect(rustFields.sort()).toEqual(["path", "summary"]);
    expect(tsFields.sort()).toEqual(rustFields.sort());
    for (const command of ["backup_export", "backup_import"]) {
      expect(shellSource, command).toMatch(new RegExp(String.raw`async fn ${command}[^{]*->\s*Result<BackupResult, ApiError>`));
    }
  });

  // --- attachments (ADR-0020) ---

  it.each([
    ["ATTACHMENT_HEADER", "x-kb-attachment"],
    ["RANGE_START_HEADER", "x-kb-range-start"],
    ["RANGE_END_HEADER", "x-kb-range-end"],
  ])("both sides name the header %s %s", (constant, header) => {
    expect(attachmentIpcSource).toContain(`pub const ${constant}: &str = "${header}";`);
    expect(transportSource, `tauri.ts does not send the header ${header}`).toMatch(new RegExp(`["'\`]${header}["'\`]`));
  });

  it("attachment_import_path takes the space and an optional path, and answers the AttachmentInfo", () => {
    expect(rustParameters("attachment_import_path").filter((parameter) => !isInjected(parameter.type))).toEqual([
      { name: "space_id", type: "SpaceId" },
      { name: "path", type: "Option<String>" },
    ]);
    expect(shellSource).toMatch(/async fn attachment_import_path[^{]*->\s*Result<AttachmentInfo, ApiError>/);
  });

  it("attachment_import_url takes the space and the link, and answers the AttachmentInfo (ADR-0028)", () => {
    expect(rustParameters("attachment_import_url").filter((parameter) => !isInjected(parameter.type))).toEqual([
      { name: "space_id", type: "SpaceId" },
      { name: "url", type: "String" },
    ]);
    expect(shellSource).toMatch(/async fn attachment_import_url[^{]*->\s*Result<AttachmentInfo, ApiError>/);
    // The transport sends both as one JSON object.
    expect(transportSource).toMatch(new RegExp(`${INVOKE}"attachment_import_url",\\s*\\{\\s*spaceId,\\s*url\\s*\\}`));
  });

  it("attachment_save_as takes the attachment and an optional path, and answers { info, path }", () => {
    expect(rustParameters("attachment_save_as").filter((parameter) => !isInjected(parameter.type))).toEqual([
      { name: "attachment_id", type: "AttachmentId" },
      { name: "path", type: "Option<String>" },
    ]);
    expect(shellSource).toMatch(/async fn attachment_save_as[^{]*->\s*Result<AttachmentSaved, ApiError>/);

    const rustStruct = /pub struct AttachmentSaved \{([^}]*)\}/.exec(attachmentsSource)?.[1];
    expect(rustStruct, "AttachmentSaved not found in src-tauri/src/attachments.rs").toBeDefined();
    const rustFields = [...(rustStruct ?? "").matchAll(/pub (\w+):/g)].map((match) => camelCase(match[1] ?? ""));
    // The struct is serialised with `rename_all = "camelCase"`; `info` is the kb-types AttachmentInfo.
    expect(attachmentsSource).toMatch(/#\[serde\(rename_all = "camelCase"\)\]\s*pub struct AttachmentSaved/);
    expect(rustFields.sort()).toEqual(["info", "path"]);
  });

  it("attachment_open takes the attachment and answers the path of the private copy as text", () => {
    expect(rustParameters("attachment_open").filter((parameter) => !isInjected(parameter.type))).toEqual([
      { name: "attachment_id", type: "AttachmentId" },
    ]);
    expect(shellSource).toMatch(/async fn attachment_open[^{]*->\s*Result<String, ApiError>/);
  });

  // --- local AI and MCP (ADR-0037) ---

  it("both sides name the transcription header x-kb-ai", () => {
    expect(aiHostSource).toContain('pub const AI_HEADER: &str = "x-kb-ai";');
    expect(transportSource, "tauri.ts does not send the header x-kb-ai").toMatch(/["'`]x-kb-ai["'`]/);
  });

  it("ai_transcribe takes the WAV as the raw request body and answers JSON, as the transport sends it", () => {
    expect(shellSource).toMatch(
      /async fn ai_transcribe\([^)]*\brequest: tauri::ipc::Request<'_>,?\s*\)\s*->\s*Result<AiTranscribed, ApiError>/,
    );
    // The bytes are the body and the options the header: `invoke("ai_transcribe", wav, { headers })`.
    expect(transportSource).toMatch(new RegExp(`${INVOKE}"ai_transcribe",\\s*wav,\\s*\\{\\s*headers`));
  });

  it("ai_generate and ai_chat take the request id, the input and the channel under the names the transport sends", () => {
    for (const [command, input] of [
      ["ai_generate", "AiGenerateInput"],
      ["ai_chat", "AiChatInput"],
    ] as const) {
      expect(rustParameters(command).filter((parameter) => !isInjected(parameter.type))).toEqual([
        { name: "request_id", type: "String" },
        { name: "input", type: input },
        { name: "on_event", type: "Channel<AiStreamEvent>" },
      ]);
      expect(shellSource).toMatch(new RegExp(String.raw`async fn ${command}[^{]*->\s*Result<\(\), ApiError>`));
      expect(transportSource).toMatch(
        new RegExp(`${INVOKE}"${command}",\\s*\\{\\s*requestId,\\s*input,\\s*onEvent:\\s*channel\\s*\\}`),
      );
    }
  });

  it("the AI and MCP commands take and answer what the transport validates", () => {
    const parameters = (command: string) => rustParameters(command).filter((parameter) => !isInjected(parameter.type));
    expect(parameters("ai_status")).toEqual([]);
    expect(parameters("mcp_status")).toEqual([]);
    expect(parameters("ai_endpoint_set")).toEqual([{ name: "url", type: "String" }]);
    expect(parameters("ai_cancel")).toEqual([{ name: "request_id", type: "String" }]);
    expect(parameters("ai_search")).toEqual([
      { name: "query", type: "String" },
      { name: "scope", type: "AiScope" },
      { name: "limit", type: "Option<u32>" },
    ]);
    expect(parameters("mcp_client_grants_set")).toEqual([
      { name: "client_id", type: "String" },
      { name: "grants", type: "Vec<McpGrant>" },
    ]);
    for (const [command, answer] of [
      ["ai_status", "AiStatus"],
      ["ai_endpoint_set", "AiStatus"],
      ["ai_cancel", "()"],
      ["ai_search", "Vec<AiSource>"],
      ["mcp_status", "McpStatus"],
      ["mcp_client_grants_set", "McpClientInfo"],
    ] as const) {
      const escaped = answer.replace(/[()<>]/g, String.raw`\$&`);
      expect(shellSource, command).toMatch(new RegExp(String.raw`async fn ${command}[^{]*->\s*Result<${escaped}, ApiError>`));
    }
  });

  // --- local dictation (ADR-0053) ---

  it("both sides name the dictation header x-kb-dictation", () => {
    expect(speechHostSource).toContain('pub const SESSION_HEADER: &str = "x-kb-dictation";');
    expect(transportSource, "dictation-tauri.ts does not send the header x-kb-dictation").toMatch(/["'`]x-kb-dictation["'`]/);
  });

  it("dictation_audio takes the samples as the raw request body and answers nothing, as the transport sends them", () => {
    expect(shellSource).toMatch(
      /async fn dictation_audio\([^)]*\brequest: tauri::ipc::Request<'_>,?\s*\)\s*->\s*Result<\(\), ApiError>/,
    );
    // The bytes are the body and the session the header: `invoke("dictation_audio", bytes, { headers })`.
    expect(transportSource).toMatch(new RegExp(`${INVOKE}"dictation_audio",\\s*bytes,\\s*\\{\\s*headers`));
  });

  it("dictation_start and dictation_install take their arguments and the channel under the names the transport sends", () => {
    const parameters = (command: string) => rustParameters(command).filter((parameter) => !isInjected(parameter.type));
    expect(parameters("dictation_start")).toEqual([
      { name: "session_id", type: "String" },
      { name: "engine", type: "SpeechEngine" },
      { name: "on_event", type: "Channel<DictationEvent>" },
    ]);
    expect(parameters("dictation_install")).toEqual([
      { name: "engine", type: "SpeechEngine" },
      { name: "on_event", type: "Channel<InstallEvent>" },
    ]);
    for (const command of ["dictation_start", "dictation_install"]) {
      expect(shellSource).toMatch(new RegExp(String.raw`async fn ${command}[^{]*->\s*Result<\(\), ApiError>`));
    }
    expect(transportSource).toMatch(
      new RegExp(`${INVOKE}"dictation_start",\\s*\\{\\s*sessionId,\\s*engine,\\s*onEvent:\\s*channel\\s*\\}`),
    );
    expect(transportSource).toMatch(
      new RegExp(`${INVOKE}"dictation_install",\\s*\\{\\s*engine,\\s*onEvent:\\s*channel\\s*\\}`),
    );
  });

  it("the other dictation commands take and answer what the transport validates", () => {
    const parameters = (command: string) => rustParameters(command).filter((parameter) => !isInjected(parameter.type));
    expect(parameters("dictation_status")).toEqual([]);
    expect(parameters("dictation_install_cancel")).toEqual([]);
    for (const command of ["dictation_select", "dictation_import", "dictation_remove"]) {
      expect(parameters(command), command).toEqual([{ name: "engine", type: "SpeechEngine" }]);
    }
    for (const command of ["dictation_stop", "dictation_cancel"]) {
      expect(parameters(command), command).toEqual([{ name: "session_id", type: "String" }]);
    }
    for (const [command, answer] of [
      ["dictation_status", "DictationStatus"],
      ["dictation_select", "DictationStatus"],
      ["dictation_import", "DictationStatus"],
      ["dictation_remove", "DictationStatus"],
      ["dictation_install_cancel", "()"],
      ["dictation_stop", "()"],
      ["dictation_cancel", "()"],
    ] as const) {
      const escaped = answer.replace(/[()<>]/g, String.raw`\$&`);
      expect(shellSource, command).toMatch(new RegExp(String.raw`async fn ${command}[^{]*->\s*Result<${escaped}, ApiError>`));
    }
  });

  // --- Google Drive backups and the restore wizard (ADR-0059) ---

  it("the Drive commands take and answer what the transport validates", () => {
    const parameters = (command: string) => rustParameters(command).filter((parameter) => !isInjected(parameter.type));
    for (const command of ["drive_status", "drive_connect", "drive_disconnect", "drive_backups_list", "drive_cancel"]) {
      expect(parameters(command), command).toEqual([]);
      expect(invokedArgumentNames(command), command).toEqual([]);
    }
    expect(parameters("drive_client_set")).toEqual([{ name: "client", type: "Option<DriveClientInput>" }]);
    expect(parameters("drive_backup_download")).toEqual([
      { name: "file_id", type: "String" },
      { name: "on_event", type: "Channel<DriveProgress>" },
    ]);
    expect(parameters("backup_folder_list")).toEqual([{ name: "path", type: "Option<String>" }]);
    for (const [command, answer] of [
      ["drive_status", "DriveStatus"],
      ["drive_connect", "DriveStatus"],
      ["drive_disconnect", "DriveStatus"],
      ["drive_client_set", "DriveStatus"],
      ["drive_backups_list", "Vec<BackupFileInfo>"],
      ["drive_backup_download", "String"],
      ["drive_cancel", "()"],
      ["backup_folder_list", "BackupFolderListing"],
    ] as const) {
      const escaped = answer.replace(/[()<>]/g, String.raw`\$&`);
      expect(shellSource, command).toMatch(new RegExp(String.raw`async fn ${command}[^{]*->\s*Result<${escaped}, ApiError>`));
    }
    // The transport sends the client, the file id and the channel under the names Tauri reads (camelCase).
    expect(transportSource).toMatch(new RegExp(`${INVOKE}"drive_client_set",\\s*\\{\\s*client\\s*\\}`));
    expect(transportSource).toMatch(new RegExp(`${INVOKE}"drive_backup_download",\\s*\\{\\s*fileId,\\s*onEvent\\s*\\}`));
  });

  it("the reasons a Drive command fails with are the words the page reads", () => {
    const list = /DRIVE_ERROR_REASONS = \[([^\]]*)\]/.exec(code(driveRaw))?.[1];
    expect(list, "DRIVE_ERROR_REASONS not found in packages/core-client/src/drive.ts").toBeDefined();
    const ts = [...(list ?? "").matchAll(/"(\w+)"/g)].map((match) => match[1] ?? "");
    const rustBody = /const fn as_str\(self\) -> &'static str \{([\s\S]*?)\n {4}\}/.exec(driveHostRaw)?.[1];
    expect(rustBody, "Reason::as_str not found in src-tauri/src/drive_host.rs").toBeDefined();
    const rust = [...(rustBody ?? "").matchAll(/=> "(\w+)",/g)].map((match) => match[1] ?? "");
    expect(rust).toHaveLength(11);
    expect(ts).toEqual(rust);
  });

  it("the shell's session rules are the ones the transport and the contract name", () => {
    // A session id is 1 to 64 of [A-Za-z0-9_-]: the shell checks it, the transport makes one that passes.
    expect(speechHostSource).toMatch(/id\.len\(\) <= 64/);
    expect(dictationTauriRaw).toContain("newSessionId");
    // The longest push (2 s of 16 kHz 16-bit samples, 64,000 bytes) is the same number on both sides.
    expect(speechHostRaw).toContain("MAX_PUSH_BYTES");
  });

  // --- the shell's own messages, deep links, the quick capture and the tray (ADR-0040) ---

  it("shell_events_subscribe takes the channel under the name the transport sends it (onEvent is on_event in Rust)", () => {
    expect(rustParameters("shell_events_subscribe").filter((parameter) => !isInjected(parameter.type))).toEqual([
      { name: "on_event", type: "Channel<ShellEvent>" },
    ]);
    expect(shellSource).toMatch(/async fn shell_events_subscribe[^{]*->\s*Result<\(\), ApiError>/);
    expect(transportSource).toMatch(new RegExp(`${INVOKE}"shell_events_subscribe",\\s*\\{\\s*onEvent:\\s*channel\\s*\\}`));
  });

  it("the link the shell sends is the shape the transport reads: { pageId, blockId? }", () => {
    const rust = deepLinkRaw.split("\n");
    const start = rust.findIndex((line) => line.startsWith("pub struct DeepLink {"));
    expect(start, "pub struct DeepLink not found in src-tauri/src/deep_link.rs").toBeGreaterThan(-1);
    const body = rust.slice(start + 1, rust.indexOf("}", start)).join("\n");
    expect([...body.matchAll(/pub (\w+):/g)].map((match) => camelCase(match[1] ?? "")).sort()).toEqual(["blockId", "pageId"]);
    expect(deepLinkRaw).toMatch(/#\[serde\(rename_all = "camelCase"\)\]\s*pub struct DeepLink/);
    const ts = /export interface DeepLink \{([^}]*)\}/.exec(transportTypesSource)?.[1];
    expect(ts, "DeepLink not found in packages/core-client/src/transport.ts").toBeDefined();
    expect([...(ts ?? "").matchAll(/^\s*(\w+)\??\s*:/gm)].map((match) => match[1] ?? "").sort()).toEqual(["blockId", "pageId"]);
  });

  it("the messages of the channel are the types the transport reads: { type, data? }", () => {
    // Adjacently tagged, in camelCase: `DeepLink` is `deepLink`.
    expect(shellEventsRaw).toMatch(/#\[serde\(tag = "type", content = "data", rename_all = "camelCase"\)\]\s*pub enum ShellEvent/);
    const rust = enumVariants(shellEventsRaw, "ShellEvent").map((name) => (name[0] ?? "").toLowerCase() + name.slice(1));
    const ts = /export type ShellEvent =([\s\S]*?)\n\s*\n/.exec(transportTypesSource)?.[1];
    expect(ts, "type ShellEvent not found in packages/core-client/src/transport.ts").toBeDefined();
    expect([...(ts ?? "").matchAll(/type: "(\w+)"/g)].map((match) => match[1] ?? "")).toEqual(rust);
  });

  it("the states of the capture shortcut are the ones the transport reads", () => {
    expect(shortcutRaw).toMatch(/#\[serde\(rename_all = "lowercase"\)\]\s*pub enum ShortcutState/);
    const rust = enumVariants(shortcutRaw, "ShortcutState").map((name) => name.toLowerCase());
    const ts = /export type ShortcutState =([^;]*);/.exec(transportTypesSource)?.[1];
    expect(ts, "type ShortcutState not found in packages/core-client/src/transport.ts").toBeDefined();
    expect([...(ts ?? "").matchAll(/"(\w+)"/g)].map((match) => match[1] ?? "")).toEqual(rust);
  });

  it("shell_labels_set takes the labels under the name the transport sends it", () => {
    expect(rustParameters("shell_labels_set").filter((parameter) => !isInjected(parameter.type))).toEqual([
      { name: "labels", type: "shell_labels::ShellLabels" },
    ]);
    expect(shellSource).toMatch(/async fn shell_labels_set[^{]*->\s*Result<\(\), ApiError>/);
    expect(transportSource).toMatch(new RegExp(`${INVOKE}"shell_labels_set",\\s*\\{\\s*labels\\s*\\}`));
  });

  it("the label ids are the ones the transport sends, and the shell refuses any other", () => {
    expect(shellLabelsRaw).toMatch(/#\[serde\(rename_all = "camelCase", deny_unknown_fields\)\]\s*pub struct ShellLabels/);
    const rust = shellLabelsRaw.split("\n");
    const start = rust.findIndex((line) => line.startsWith("pub struct ShellLabels {"));
    expect(start, "pub struct ShellLabels not found in src-tauri/src/shell_labels.rs").toBeGreaterThan(-1);
    const body = rust.slice(start + 1, rust.indexOf("}", start)).join("\n");
    const ids = [...body.matchAll(/pub (\w+): Option<String>/g)].map((match) => camelCase(match[1] ?? ""));
    const ts = /export interface ShellLabels \{([^}]*)\}/.exec(transportTypesSource)?.[1];
    expect(ts, "ShellLabels not found in packages/core-client/src/transport.ts").toBeDefined();
    expect([...(ts ?? "").matchAll(/^\s*(\w+)\?\s*:/gm)].map((match) => match[1] ?? "")).toEqual(ids);
  });

  it("the shell's English defaults for the tray are the strings of en.json that the UI sends", () => {
    const en = JSON.parse(enRaw) as Record<string, string>;
    const rustDefault = (name: string): string | undefined =>
      new RegExp(String.raw`pub const ${name}: &str = "([^"]*)";`).exec(shellLabelsRaw)?.[1];
    expect(rustDefault("DEFAULT_OPEN")).toBe(en["tray.open"]);
    expect(rustDefault("DEFAULT_NEW_PAGE")).toBe(en["tray.newPage"]);
    expect(rustDefault("DEFAULT_LOCK")).toBe(en["tray.lock"]);
    expect(rustDefault("DEFAULT_QUIT")).toBe(en["tray.quit"]);
  });

  // --- presence in shared spaces (M13 slice 3, ADR-0050) ---

  it("presence_send takes the space, the page, the state and the update under the names the transport sends, and answers nothing", () => {
    expect(rustParameters("presence_send").filter((parameter) => !isInjected(parameter.type))).toEqual([
      { name: "space_id", type: "SpaceId" },
      { name: "page_id", type: "Option<PageId>" },
      { name: "state", type: "PresenceState" },
      { name: "update", type: "Vec<u8>" },
    ]);
    expect(shellSource).toMatch(/async fn presence_send[^{]*->\s*Result<\(\), ApiError>/);
    // The update travels as an array of byte values (a JSON number array), never as a raw body.
    expect(transportSource).toMatch(new RegExp(`${INVOKE}"presence_send",\\s*\\{\\s*spaceId,\\s*pageId,\\s*state,\\s*update:\\s*bytes\\s*\\}`));
  });

  it("presence_events takes the channel under the name the transport sends it (onEvent is on_event in Rust)", () => {
    expect(rustParameters("presence_events").filter((parameter) => !isInjected(parameter.type))).toEqual([
      { name: "on_event", type: "Channel<PresenceMessage>" },
    ]);
    expect(shellSource).toMatch(/async fn presence_events[^{]*->\s*Result<\(\), ApiError>/);
    expect(transportSource).toMatch(new RegExp(`${INVOKE}"presence_events",\\s*\\{\\s*onEvent:\\s*channel\\s*\\}`));
  });

  it("the presence states the transport accepts are the variants of kb-types' PresenceState", () => {
    const generated = [...code(presenceStateRaw).matchAll(/"(\w+)"/g)].map((match) => match[1] ?? "");
    expect(generated).toEqual(["Viewing", "Editing", "Idle"]);
    const accepted = /const PRESENCE_STATES[^=]*=\s*new Set<PresenceState>\(\[([^\]]*)\]\)/.exec(transportSource)?.[1];
    expect(accepted, "PRESENCE_STATES not found in packages/core-client/src/tauri.ts").toBeDefined();
    expect([...(accepted ?? "").matchAll(/"(\w+)"/g)].map((match) => match[1] ?? "")).toEqual(generated);
  });

  // --- the native menu bar (ADR-0040 item 12) ---

  it("menu_bar_show and menu_bar_hide take nothing from the webview and answer nothing, as the transport calls them", () => {
    for (const command of ["menu_bar_show", "menu_bar_hide"]) {
      expect(rustParameters(command).filter((parameter) => !isInjected(parameter.type)), command).toEqual([]);
      expect(shellSource, command).toMatch(new RegExp(String.raw`async fn ${command}[^{]*->\s*Result<\(\), ApiError>`));
      expect(invokedArgumentNames(command), command).toEqual([]);
      expect(transportSource, command).toMatch(new RegExp(`${INVOKE}"${command}"\\s*\\)`));
    }
  });

  it("the shell's English defaults for the menu bar are the menu.* strings of en.json that the UI sends", () => {
    const en = JSON.parse(enRaw) as Record<string, string>;
    const ts = /export interface ShellLabels \{([^}]*)\}/.exec(transportTypesSource)?.[1] ?? "";
    const ids = [...ts.matchAll(/^\s*(menu\w+)\?\s*:/gm)].map((match) => match[1] ?? "");
    expect(ids).toHaveLength(24);
    for (const id of ids) {
      // `menuNewPage` is `menu.newPage` and `DEFAULT_MENU_NEW_PAGE`.
      const key = `menu.${id.charAt(4).toLowerCase()}${id.slice(5)}`;
      const constant = `DEFAULT_${id.replace(/([A-Z])/g, "_$1").toUpperCase()}`;
      const rust = new RegExp(String.raw`pub const ${constant}: &str = "([^"]*)";`).exec(shellLabelsRaw)?.[1];
      expect(rust, constant).toBeDefined();
      // The Rust source writes the ellipsis as \u{2026}.
      expect(rust?.replace(/\\u\{2026\}/g, "…"), id).toBe(en[key]);
    }
  });

  it("the shell's English defaults for the dialogs and the file types are the shellDialog.* and fileFilter.* strings of en.json that the UI sends", () => {
    const en = JSON.parse(enRaw) as Record<string, string>;
    const ts = /export interface ShellLabels \{([^}]*)\}/.exec(transportTypesSource)?.[1] ?? "";
    const ids = [...ts.matchAll(/^\s*((?:dialog|fileFilter)\w+)\?\s*:/gm)].map((match) => match[1] ?? "");
    // 29 before the Drive notice (ADR-0059) added its five.
    expect(ids).toHaveLength(34);
    const lowerFirst = (text: string): string => text.charAt(0).toLowerCase() + text.slice(1);
    for (const id of ids) {
      // `dialogAiTitle` is `shellDialog.aiTitle` and `DEFAULT_DIALOG_AI_TITLE`; `fileFilterZip` is `fileFilter.zip`.
      const key = id.startsWith("dialog") ? `shellDialog.${lowerFirst(id.slice(6))}` : `fileFilter.${lowerFirst(id.slice(10))}`;
      const constant = `DEFAULT_${id.replace(/([A-Z])/g, "_$1").toUpperCase()}`;
      const rust = new RegExp(String.raw`pub const ${constant}: &str = "([^"]*)";`).exec(shellLabelsRaw)?.[1];
      expect(rust, constant).toBeDefined();
      expect(rust, id).toBe(en[key]);
    }
  });

  it("the consent dialogs keep the placeholders the shell fills, in the Rust defaults and in en.json alike", () => {
    // The shell requires these in every translation (shell_labels.rs `TemplateSpec`); the UI's English has them.
    const en = JSON.parse(enRaw) as Record<string, string>;
    const required: Record<string, string[]> = {
      "shellDialog.aiMessage": ["host"],
      "shellDialog.mcpRead": ["client", "spaces"],
      "shellDialog.mcpWrite": ["client", "spaces"],
      "shellDialog.mcpBoth": ["client", "read", "write"],
      "shellDialog.listAnd": ["names", "last"],
      "shellDialog.listMore": ["count"],
      "shellDialog.imagesOne": ["stored"],
      "shellDialog.imagesOther": ["count", "stored"],
      "shellDialog.hostsOne": ["host"],
      "shellDialog.hostsOther": ["host", "count"],
      // The body of the OS notification of a comment or a mention names the person (ADR-0048).
      "shellDialog.notifyThreadReply": ["actor"],
      "shellDialog.notifyCommentMention": ["actor"],
      "shellDialog.notifyPageMention": ["actor"],
      "shellDialog.notifyPersonAssigned": ["actor"],
      // The download of a speech model for dictation says how much and from where (ADR-0053); `{model}` is optional.
      "shellDialog.dictationMessage": ["size", "host"],
      // The notice before Google Drive is connected and the page of the sign-in's end are plain words (ADR-0059).
      "shellDialog.driveMessage": [],
      "shellDialog.driveDone": [],
    };
    for (const [key, names] of Object.entries(required)) {
      for (const name of names) {
        expect(en[key], key).toContain(`{${name}}`);
      }
      // The Rust side declares the same set for the field.
      const field = key.replace("shellDialog.", "dialog").replace(/^dialog(.)/, (_, first: string) => `dialog${first.toUpperCase()}`);
      const spec = new RegExp(String.raw`field: "${field}",\s*allowed: &\[([^\]]*)\],\s*required: &\[([^\]]*)\]`).exec(shellLabelsRaw);
      expect(spec, field).not.toBeNull();
      const listed = [...(spec?.[2] ?? "").matchAll(/"(\w+)"/g)].map((match) => match[1] ?? "");
      expect(listed.sort(), field).toEqual([...names].sort());
    }
  });

  it("the commands of a menu message are the ones the transport reads", () => {
    expect(menuBarRaw).toMatch(/#\[serde\(rename_all = "camelCase"\)\]\s*pub enum UiCommand/);
    const rust = enumVariants(menuBarRaw, "UiCommand").map((name) => (name[0] ?? "").toLowerCase() + name.slice(1));
    const ts = /export type MenuCommand =([^;]*);/.exec(transportTypesSource)?.[1];
    expect(ts, "type MenuCommand not found in packages/core-client/src/transport.ts").toBeDefined();
    expect([...(ts ?? "").matchAll(/"(\w+)"/g)].map((match) => match[1] ?? "")).toEqual(rust);
    // And the transport's list of the ones it accepts.
    const accepted = /const MENU_COMMANDS[^=]*=\s*new Set<MenuCommand>\(\[([^\]]*)\]\)/.exec(transportSource)?.[1];
    expect(accepted, "MENU_COMMANDS not found in packages/core-client/src/tauri.ts").toBeDefined();
    expect([...(accepted ?? "").matchAll(/"(\w+)"/g)].map((match) => match[1] ?? "")).toEqual(rust);
  });

  it("the shortcut hints of the menu are the keys the UI binds", () => {
    const hint = (name: string): string | undefined =>
      new RegExp(String.raw`pub const ${name}: &str = "([^"]*)";`).exec(menuBarRaw)?.[1];
    // Back and forward: the platform's own keys of `shortcuts.ts` (Alt+Left and Alt+Right off macOS).
    const ui = (name: string): string | undefined =>
      new RegExp(String.raw`export const ${name} = IS_MAC \? "[^"]*" : "([^"]*)";`).exec(shortcutsRaw)?.[1];
    expect(hint("HINT_BACK")).toBe(ui("BACK_SHORTCUT"));
    expect(hint("HINT_FORWARD")).toBe(ui("FORWARD_SHORTCUT"));
    expect(hint("HINT_BACK")).toBe("Alt+Left");
    expect(hint("HINT_FORWARD")).toBe("Alt+Right");
    // Print, find in page and the sidebar: the bindings are in the sources that own them.
    expect(hint("HINT_PRINT")).toBe("Ctrl+P");
    expect(printRaw).toContain('event.key.toLowerCase() === "p"');
    expect(hint("HINT_FIND")).toBe("Ctrl+F");
    expect(findBarRaw).toContain('event.key.toLowerCase() !== "f"');
    expect(hint("HINT_TOGGLE_SIDEBAR")).toBe(String.raw`Ctrl+\\`);
    expect(mainLayoutRaw).toContain('event.key === "\\\\" || (event.code === "Backslash"');
    // Close window is the system's own key, which Windows runs as a close request.
    expect(hint("HINT_CLOSE_WINDOW")).toBe("Alt+F4");
  });

  it("the menu is built with no accelerator: a hint is text, never a registered key", () => {
    // Every menu item of ours is made with no accelerator argument, so nothing is added to the window's table.
    const code = (menuBarRaw.split("#[cfg(test)]")[0] ?? "").replace(/^\s*\/\/.*$/gm, "");
    expect(code).toContain("MenuItem::with_id(app, item.id(), shown, true, None::<&str>)");
    expect(code).not.toMatch(/accelerator\s*\(/);
    expect(code).not.toMatch(/Some\("(?:Ctrl|Alt|Shift|Cmd|CmdOrCtrl)/);
    // And the tab is the platform's own column, added in one place.
    expect(code).toContain('format!("{label}\\t{hint}")');
  });

  it("the two device settings and the default shortcut are spelled the same in the shell and in the UI", () => {
    const rustConst = (name: string): string | undefined =>
      new RegExp(String.raw`pub const ${name}: &str = "([^"]*)";`).exec(desktopSettingsRaw)?.[1];
    const tsConst = (source: string, name: string): string | undefined =>
      new RegExp(String.raw`export const ${name} = "([^"]*)";`).exec(source)?.[1];
    expect(rustConst("CLOSE_TO_TRAY_SETTING")).toBe("app.tray.close_to_tray");
    expect(tsConst(storeShellRaw, "CLOSE_TO_TRAY_KEY")).toBe(rustConst("CLOSE_TO_TRAY_SETTING"));
    expect(rustConst("CAPTURE_SHORTCUT_SETTING")).toBe("app.shortcut.capture");
    expect(tsConst(storeShellRaw, "CAPTURE_SHORTCUT_KEY")).toBe(rustConst("CAPTURE_SHORTCUT_SETTING"));
    expect(rustConst("DEFAULT_CAPTURE_SHORTCUT")).toBe("Ctrl+Alt+N");
    expect(tsConst(uiAcceleratorRaw, "DEFAULT_CAPTURE_SHORTCUT")).toBe(rustConst("DEFAULT_CAPTURE_SHORTCUT"));
  });

  it("the named keys the Settings recorder writes are the ones the shell's accelerator reads", () => {
    const rustBody = /fn canonical_key[\s\S]*?\n\}\n/.exec(acceleratorRaw)?.[0];
    expect(rustBody, "fn canonical_key not found in src-tauri/src/accelerator.rs").toBeDefined();
    const rust = new Set([...(rustBody ?? "").matchAll(/=> "(\w+)",/g)].map((match) => match[1] ?? ""));
    const list = /const NAMED_KEYS[^=]*=\s*new Set\(\[([^\]]*)\]\)/.exec(uiAcceleratorRaw)?.[1];
    expect(list, "NAMED_KEYS not found in packages/ui/src/accelerator.ts").toBeDefined();
    const ui = new Set([...(list ?? "").matchAll(/"(\w+)"/g)].map((match) => match[1] ?? ""));
    expect([...ui].sort()).toEqual([...rust].sort());
    expect(rust.size).toBeGreaterThan(20);
  });

  // --- every JSON command, generically ---

  it.each(JSON_COMMANDS)("%s: the transport sends exactly the arguments the Rust command takes", (command) => {
    const rust = rustParameters(command)
      .filter((parameter) => !isInjected(parameter.type))
      .map((parameter) => camelCase(parameter.name))
      .sort();
    expect(invokedArgumentNames(command).sort()).toEqual(rust);
  });
});
