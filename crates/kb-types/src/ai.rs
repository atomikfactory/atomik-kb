//! Local AI and the MCP server (ADR-0037, `docs/architecture/ai.md`): settings, the data of the
//! writing assistant, the chat and the transcription (shell commands), the semantic index and
//! retrieval (core requests), and the MCP clients, grants and audit. Pure data; the rules live in
//! `kb-ai` and `kb-mcp`, the index and the clients in `kb-core`, the network in the shell.

use serde::{Deserialize, Serialize};

use crate::api::ApiError;
use crate::ids::{BlockId, PageId, SpaceId};

// ----- settings -----------------------------------------------------------------------------

/// Device setting holding the [`AiEndpoint`]. Written only by the desktop shell
/// (`ai_endpoint_set`, with a native consent dialog for a remote host); the shell refuses a
/// `SettingsSet` of this key coming from the webview.
pub const AI_ENDPOINT_SETTING: &str = "app.ai.endpoint";

/// Device setting holding the [`AiSettings`] (written by the UI).
pub const AI_SETTINGS_SETTING: &str = "app.ai.settings";

/// Device setting holding the [`McpSettings`] (written by the UI).
pub const MCP_SETTINGS_SETTING: &str = "app.mcp.settings";

/// The Ollama endpoint used when none is stored.
pub const DEFAULT_AI_ENDPOINT: &str = "http://127.0.0.1:11434";

/// The loopback port of the MCP server when none is stored.
pub const DEFAULT_MCP_PORT: u16 = 47_811;

/// The Ollama endpoint (setting [`AI_ENDPOINT_SETTING`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AiEndpoint {
    /// Normalised base URL (`http(s)://host[:port]`, no trailing slash).
    pub url: String,
    /// The person allowed text to leave this device for this exact URL (only meaningful for a
    /// remote host; a local one never needs it).
    #[serde(default)]
    pub remote_consent: bool,
}

/// AI preferences (setting [`AI_SETTINGS_SETTING`]). Absent = AI off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AiSettings {
    /// Master switch; every AI feature is hidden and refused while it is off.
    #[serde(default)]
    pub enabled: bool,
    /// Ollama model for writing and chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub chat_model: Option<String>,
    /// Ollama model for the semantic index and the questions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub embedding_model: Option<String>,
    /// Ollama model that understands audio (transcription).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub transcription_model: Option<String>,
    /// Build and use the semantic index (default true).
    #[serde(default = "default_true")]
    pub semantic_index: bool,
    /// Let models that support it think before answering (default false).
    #[serde(default)]
    pub think: bool,
}

const fn default_true() -> bool {
    true
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            chat_model: None,
            embedding_model: None,
            transcription_model: None,
            semantic_index: true,
            think: false,
        }
    }
}

/// MCP server preferences (setting [`MCP_SETTINGS_SETTING`]). Absent = off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct McpSettings {
    /// The loopback server runs while the app is open and unlocked.
    #[serde(default)]
    pub enabled: bool,
    /// Port on 127.0.0.1 (1024 to 65535; default [`DEFAULT_MCP_PORT`]).
    #[serde(default = "default_mcp_port")]
    pub port: u16,
}

const fn default_mcp_port() -> u16 {
    DEFAULT_MCP_PORT
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_MCP_PORT,
        }
    }
}

// ----- status (shell command `ai_status`) -------------------------------------------------

/// A model installed in Ollama.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AiModelInfo {
    /// Model name with its tag (`qwen3:8b`).
    pub name: String,
    /// Family (`qwen3`), when Ollama says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub family: Option<String>,
    /// Parameter size (`8.2B`), when Ollama says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub parameter_size: Option<String>,
    /// Ollama's capabilities: `completion`, `embedding`, `audio`, `vision`, `thinking`, `tools`.
    pub capabilities: Vec<String>,
    /// Size on disk, bytes.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub size: u64,
}

/// What the background indexer is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum AiIndexerPhase {
    /// Nothing to do, or AI or the index is off.
    #[default]
    Idle,
    /// Embedding pages.
    Indexing,
    /// Waiting after a failure (Ollama unreachable, model missing).
    Error,
}

/// The indexer's state, kept by the shell.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AiIndexerState {
    /// Phase.
    pub phase: AiIndexerPhase,
    /// The last failure, a short sentence without page text or host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub error: Option<String>,
    /// When a batch was last stored, Unix ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub last_run_at: Option<i64>,
}

/// Reply of the shell commands `ai_status` and `ai_endpoint_set`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AiStatus {
    /// The endpoint in use.
    pub endpoint: String,
    /// Its host is this machine (loopback).
    pub local: bool,
    /// Consent to send text to a remote endpoint was given for it.
    pub remote_consent: bool,
    /// Ollama answered.
    pub reachable: bool,
    /// Ollama's version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub version: Option<String>,
    /// Installed models (empty when unreachable).
    pub models: Vec<AiModelInfo>,
    /// Why Ollama could not be reached, or the consent that is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub error: Option<ApiError>,
    /// The semantic index, when the core is ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub index: Option<AiIndexInfo>,
    /// The background indexer.
    pub indexer: AiIndexerState,
}

// ----- writing assistant and chat (shell commands `ai_generate`, `ai_chat`) ------------------

/// What the writing assistant does with the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum AiAction {
    /// Continue writing after the context.
    Continue,
    /// Improve clarity, flow and word choice.
    Improve,
    /// Fix spelling, grammar and punctuation only.
    FixGrammar,
    /// About half as long.
    Shorter,
    /// About twice as long.
    Longer,
    /// Plain, simple language.
    Simplify,
    /// Another tone ([`AiGenerateInput::tone`]).
    Tone,
    /// Another language ([`AiGenerateInput::language`]).
    Translate,
    /// A short summary and key points.
    Summarize,
    /// Explain in simple terms.
    Explain,
    /// The action items as a task list.
    ActionItems,
    /// The person's own instruction ([`AiGenerateInput::prompt`]).
    Custom,
    /// Summary, key points and action items of a meeting transcript.
    MeetingSummary,
}

/// Tones of [`AiAction::Tone`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum AiTone {
    /// Professional.
    Professional,
    /// Casual.
    Casual,
    /// Friendly.
    Friendly,
    /// Confident.
    Confident,
    /// Direct.
    Direct,
}

/// Input of `ai_generate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AiGenerateInput {
    /// What to do.
    pub action: AiAction,
    /// The selected text as Markdown (at most 32,000 characters; may be empty for `continue` and
    /// `custom`).
    pub text: String,
    /// The page title and the text before the selection (at most 8,000 characters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub context: Option<String>,
    /// Target language of `translate` (a language name, at most 60 characters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub language: Option<String>,
    /// Tone of `tone`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub tone: Option<AiTone>,
    /// Instruction of `custom` (at most 2,000 characters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub prompt: Option<String>,
}

/// Who wrote a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum AiRole {
    /// The person.
    User,
    /// The model.
    Assistant,
}

/// One message of a chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AiChatMessage {
    /// Author.
    pub role: AiRole,
    /// Text (Markdown), at most 16,000 characters.
    pub content: String,
}

/// Where the chat, the retrieval and the related search look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AiScope {
    /// One space.
    Space {
        /// Space.
        space_id: SpaceId,
    },
    /// Every space.
    All,
    /// One page.
    Page {
        /// Page.
        page_id: PageId,
    },
}

/// Input of `ai_chat`: the conversation so far, ending with the person's question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AiChatInput {
    /// At most 40 messages, the last one from the person.
    pub messages: Vec<AiChatMessage>,
    /// Where to look.
    pub scope: AiScope,
}

/// A passage of a page, found by `AiRetrieve` and cited by a chat answer as `[n]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AiSource {
    /// Citation number, from 1.
    pub n: u32,
    /// Page.
    pub page_id: PageId,
    /// Its space.
    pub space_id: SpaceId,
    /// First block of the passage (`None`: the title).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub block_id: Option<BlockId>,
    /// Page title.
    pub title: String,
    /// Page icon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub icon: Option<String>,
    /// The passage (at most 1,500 characters).
    pub text: String,
    /// Fused rank score (higher is better; only the order matters).
    pub score: f32,
}

/// What a stream of `ai_generate` or `ai_chat` sends over its channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AiStreamEvent {
    /// The sources of a chat answer (first, chat only).
    Sources {
        /// Sources, numbered from 1.
        sources: Vec<AiSource>,
    },
    /// More text (raw tokens; the receiver appends them).
    Delta {
        /// Text.
        text: String,
    },
    /// Finished.
    Done {
        /// The whole answer, cleaned (`<think>` blocks and a wrapping fence removed).
        text: String,
        /// The model that answered.
        model: String,
    },
    /// Failed or cancelled (`details.reason = "cancelled"`). Nothing follows.
    Error {
        /// What went wrong.
        error: ApiError,
    },
}

/// Reply of `ai_transcribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AiTranscribed {
    /// The segment's speech, cleaned (empty for silence).
    pub text: String,
    /// The language the model reported, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub language: Option<String>,
}

// ----- the semantic index (core requests) ----------------------------------------------------

/// Reply of `AiIndexStatus`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AiIndexInfo {
    /// Embedding model of the stored vectors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub model: Option<String>,
    /// Live pages whose vectors are current.
    pub pages_indexed: u32,
    /// Live pages.
    pub pages_total: u32,
    /// Stored chunks.
    pub chunks: u32,
    /// Pages waiting to be (re)indexed.
    pub dirty: u32,
    /// Vector dimension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub dimensions: Option<u32>,
}

/// One chunk of a page to index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AiIndexChunk {
    /// Position in the page, from 0.
    pub seq: u32,
    /// `blake3(model, text)` in lower-case hex.
    pub hash: String,
    /// The text to embed (title line and chunk), without the model's prefix.
    pub text: String,
    /// No stored vector has this hash: the shell must embed it.
    pub needs_vector: bool,
}

/// A page whose vectors are stale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AiIndexPage {
    /// Page.
    pub page_id: PageId,
    /// Hash of the chunk hashes, lower-case hex; `AiIndexStore` must send it back.
    pub source_hash: String,
    /// Every chunk of the page, in order.
    pub chunks: Vec<AiIndexChunk>,
}

/// Reply of `AiIndexNext`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AiIndexWork {
    /// The embedding model to use; `None` when indexing is off (then `pages` is empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub model: Option<String>,
    /// Pages to embed.
    pub pages: Vec<AiIndexPage>,
    /// Dirty pages left after these.
    pub remaining: u32,
}

/// A vector computed by the shell for `AiIndexStore`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AiChunkVector {
    /// Chunk position.
    pub seq: u32,
    /// The embedding (any length 16 to 8,192; normalised by the core).
    pub vector: Vec<f32>,
}

// ----- MCP --------------------------------------------------------------------------------------

/// What one MCP client may do in one space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct McpGrant {
    /// Space.
    pub space_id: SpaceId,
    /// Search, list and read.
    pub read: bool,
    /// Create pages and append to them (implies read).
    pub write: bool,
}

/// An MCP client (an outside AI tool with its own token).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct McpClientInfo {
    /// Client id (32 lower-case hex characters).
    pub id: String,
    /// Name the person gave it.
    pub name: String,
    /// What it may do, per space (spaces without a grant are invisible to it).
    pub grants: Vec<McpGrant>,
    /// Creation time, Unix ms.
    pub created_at: i64,
    /// Last authenticated call, Unix ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub last_used_at: Option<i64>,
}

/// Reply of `McpClientCreate`: the token is shown once and never stored in clear.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct McpClientCreated {
    /// The client.
    pub client: McpClientInfo,
    /// Its bearer token (`kbm_` + 43 base64url characters).
    pub token: String,
}

/// Outcome of an audited MCP call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum McpOutcome {
    /// Done.
    Ok,
    /// The client lacks the grant.
    Denied,
    /// Bad arguments.
    Invalid,
    /// Failed in the core.
    Error,
}

/// One row of the MCP audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct McpAuditEntry {
    /// Row id (increasing).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub id: i64,
    /// Time, Unix ms.
    pub at: i64,
    /// Client id.
    pub client_id: String,
    /// Client name at the time.
    pub client_name: String,
    /// Tool name.
    pub tool: String,
    /// Space concerned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub space_id: Option<SpaceId>,
    /// Page concerned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub page_id: Option<PageId>,
    /// The query or the page title (at most 200 characters; never page text).
    pub summary: String,
    /// Outcome.
    pub outcome: McpOutcome,
}

/// Reply of `McpCall`: the JSON-RPC answer, or `None` for a notification (HTTP 202).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct McpReply {
    /// JSON-RPC response text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub body: Option<String>,
}

/// Reply of the shell command `mcp_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    /// The setting is on.
    pub enabled: bool,
    /// The server is listening.
    pub running: bool,
    /// Port.
    pub port: u16,
    /// `http://127.0.0.1:<port>/mcp`.
    pub url: String,
    /// The stdio bridge executable next to the app, when it exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub bridge_path: Option<String>,
    /// Why it is not running (port in use, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub error: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn absent_fields_take_their_defaults() {
        let settings: AiSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, AiSettings::default());
        assert!(settings.semantic_index);
        let mcp: McpSettings = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert_eq!(mcp.port, DEFAULT_MCP_PORT);
    }

    #[test]
    fn scopes_and_stream_events_are_tagged_by_type() {
        let scope = serde_json::to_value(AiScope::All).unwrap();
        assert_eq!(scope, serde_json::json!({ "type": "all" }));
        let delta = serde_json::to_value(AiStreamEvent::Delta { text: "x".into() }).unwrap();
        assert_eq!(delta, serde_json::json!({ "type": "delta", "text": "x" }));
    }
}
