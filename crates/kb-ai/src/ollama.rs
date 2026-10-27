//! Ollama's HTTP API as data (`docs/architecture/ai.md`, section 3.1): request and response
//! types of `/api/version`, `/api/tags`, `/api/chat` and `/api/embed`, and a sans-IO decoder of
//! the newline-delimited JSON of a streamed chat.

use kb_types::ai::AiModelInfo;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::util::truncate_chars;

/// Longest line the decoder accepts.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// Longest error text kept from Ollama, in characters.
pub const MAX_ERROR_CHARS: usize = 300;

/// A failure reading Ollama's answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AiWireError {
    /// Not the JSON expected.
    #[error("unexpected answer from Ollama")]
    Malformed,
    /// A line longer than [`MAX_LINE_BYTES`].
    #[error("an answer line from Ollama is too long")]
    LineTooLong,
    /// Ollama reported an error (its text, at most 300 characters).
    #[error("Ollama: {0}")]
    Remote(String),
}

/// `GET /api/version`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionResponse {
    /// Version.
    pub version: String,
}

/// A chat message on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireMessage {
    /// `system`, `user` or `assistant`.
    pub role: String,
    /// Text.
    pub content: String,
    /// Base64 images or audio (WAV) for multimodal models.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

impl WireMessage {
    /// A text message without attachments.
    #[must_use]
    pub fn text(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_owned(),
            content: content.into(),
            images: Vec::new(),
        }
    }
}

/// `options` of a chat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ChatOptions {
    /// Sampling temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Context window, tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_ctx: Option<u32>,
    /// Most tokens to generate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_predict: Option<i32>,
}

/// `POST /api/chat`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    /// Model name.
    pub model: String,
    /// Conversation.
    pub messages: Vec<WireMessage>,
    /// Stream NDJSON chunks.
    pub stream: bool,
    /// Thinking switch; sent only to models with the `thinking` capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub think: Option<bool>,
    /// Options.
    pub options: ChatOptions,
}

/// One line of a streamed chat (or the whole answer of a non-streamed one).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChatChunk {
    /// New answer text.
    pub content: String,
    /// New thinking text (not shown).
    pub thinking: String,
    /// Last chunk.
    pub done: bool,
    /// Why it stopped (`stop`, `length`, ...).
    pub done_reason: Option<String>,
}

#[derive(Deserialize)]
struct RawChatChunk {
    #[serde(default)]
    message: Option<RawChatMessage>,
    #[serde(default)]
    done: Option<bool>,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Deserialize)]
struct RawChatMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
}

/// The text of an `error` member, at most [`MAX_ERROR_CHARS`] characters.
fn remote_error(error: &Value) -> AiWireError {
    let text = match error {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let text = truncate_chars(text.trim(), MAX_ERROR_CHARS).trim_end();
    if text.is_empty() {
        AiWireError::Remote("unknown error".to_owned())
    } else {
        AiWireError::Remote(text.to_owned())
    }
}

/// Parses one line of `/api/chat` output; `{"error": ...}` is [`AiWireError::Remote`] (its
/// message cut to 300 characters).
///
/// A line must be a JSON object with a `message`, a `done` or an `error` member.
///
/// # Errors
///
/// [`AiWireError::Malformed`] or [`AiWireError::Remote`].
pub fn parse_chat_chunk(line: &str) -> Result<ChatChunk, AiWireError> {
    if !line.trim_start().starts_with('{') {
        return Err(AiWireError::Malformed);
    }
    let raw: RawChatChunk = serde_json::from_str(line).map_err(|_| AiWireError::Malformed)?;
    if let Some(error) = &raw.error {
        return Err(remote_error(error));
    }
    if raw.message.is_none() && raw.done.is_none() {
        return Err(AiWireError::Malformed);
    }
    let (content, thinking) = raw
        .message
        .map(|message| {
            (
                message.content.unwrap_or_default(),
                message.thinking.unwrap_or_default(),
            )
        })
        .unwrap_or_default();
    Ok(ChatChunk {
        content,
        thinking,
        done: raw.done.unwrap_or(false),
        done_reason: raw.done_reason,
    })
}

/// `POST /api/embed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedRequest {
    /// Model name.
    pub model: String,
    /// Texts (with the model's prefix already applied).
    pub input: Vec<String>,
    /// Let Ollama truncate texts longer than the model's context.
    pub truncate: bool,
}

#[derive(Deserialize)]
struct RawEmbed {
    #[serde(default)]
    embeddings: Option<Vec<Vec<f32>>>,
    #[serde(default)]
    error: Option<Value>,
}

/// Parses `/api/embed` output: one vector per input, in order. Every vector must be non-empty
/// and all must have the same length.
///
/// # Errors
///
/// [`AiWireError::Malformed`] or [`AiWireError::Remote`].
pub fn parse_embed(body: &str) -> Result<Vec<Vec<f32>>, AiWireError> {
    let raw: RawEmbed = serde_json::from_str(body).map_err(|_| AiWireError::Malformed)?;
    if let Some(error) = &raw.error {
        return Err(remote_error(error));
    }
    let embeddings = raw.embeddings.ok_or(AiWireError::Malformed)?;
    let width = embeddings.first().map_or(0, Vec::len);
    if !embeddings.is_empty() && (width == 0 || embeddings.iter().any(|v| v.len() != width)) {
        return Err(AiWireError::Malformed);
    }
    Ok(embeddings)
}

#[derive(Deserialize)]
struct RawTags {
    #[serde(default)]
    models: Option<Vec<RawModel>>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Deserialize)]
struct RawModel {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    details: Option<RawDetails>,
    #[serde(default)]
    capabilities: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct RawDetails {
    #[serde(default)]
    family: Option<String>,
    #[serde(default)]
    parameter_size: Option<String>,
}

/// Parses `/api/tags` output into model descriptions (capabilities derived when absent or empty:
/// a name containing `embed` is `embedding`, any other `completion`). A model with neither
/// `name` nor `model` is skipped; the order of Ollama's answer is kept.
///
/// # Errors
///
/// [`AiWireError::Malformed`] or [`AiWireError::Remote`].
pub fn parse_tags(body: &str) -> Result<Vec<AiModelInfo>, AiWireError> {
    let raw: RawTags = serde_json::from_str(body).map_err(|_| AiWireError::Malformed)?;
    if let Some(error) = &raw.error {
        return Err(remote_error(error));
    }
    let models = raw.models.ok_or(AiWireError::Malformed)?;
    Ok(models
        .into_iter()
        .filter_map(|model| {
            let name = model.name.or(model.model).filter(|name| !name.is_empty())?;
            let capabilities = match model.capabilities {
                Some(list) if !list.is_empty() => list,
                _ => vec![derived_capability(&name).to_owned()],
            };
            let (family, parameter_size) = model
                .details
                .map(|details| (details.family, details.parameter_size))
                .unwrap_or_default();
            Some(AiModelInfo {
                name,
                family: family.filter(|text| !text.is_empty()),
                parameter_size: parameter_size.filter(|text| !text.is_empty()),
                capabilities,
                size: model.size.unwrap_or(0),
            })
        })
        .collect())
}

/// What a model without a `capabilities` list is taken for: an embedding model when its name
/// says so, a completion model otherwise.
fn derived_capability(name: &str) -> &'static str {
    if name.to_ascii_lowercase().contains("embed") {
        "embedding"
    } else {
        "completion"
    }
}

/// The context length Ollama reports for a model in `/api/show` (`model_info.*.context_length`),
/// when present: the key of the model's `general.architecture` first (`llama.context_length`),
/// otherwise the first key ending in `.context_length`. Zero is not a length.
#[must_use]
pub fn context_length_of_show(body: &str) -> Option<u32> {
    let root: Value = serde_json::from_str(body).ok()?;
    let info = root.get("model_info")?.as_object()?;
    let by_architecture = info
        .get("general.architecture")
        .and_then(Value::as_str)
        .and_then(|architecture| info.get(&format!("{architecture}.context_length")));
    let found = by_architecture.or_else(|| {
        info.iter()
            .find(|(key, _)| key.ends_with(".context_length"))
            .map(|(_, value)| value)
    })?;
    u32::try_from(found.as_u64()?)
        .ok()
        .filter(|length| *length > 0)
}

/// Splits a byte stream into lines without I/O: push what arrives, take complete lines.
///
/// Lines end with `\n` or `\r\n`; blank lines are skipped. Bytes are only decoded as UTF-8 once
/// a line is complete, so a character split across two pushes is no problem. A line longer than
/// [`MAX_LINE_BYTES`] fails the stream for good.
#[derive(Debug, Default)]
pub struct NdjsonDecoder {
    buffer: Vec<u8>,
    /// Start of the unread part of `buffer`.
    start: usize,
    /// Bytes after the last `\n` seen (the unfinished line).
    tail: usize,
    /// A line was too long: every later call fails.
    overflowed: bool,
}

impl NdjsonDecoder {
    /// An empty decoder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends received bytes (a UTF-8 character may be split across pushes).
    ///
    /// # Errors
    ///
    /// [`AiWireError::LineTooLong`] when a line (finished or not) passes [`MAX_LINE_BYTES`].
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), AiWireError> {
        if self.overflowed {
            return Err(AiWireError::LineTooLong);
        }
        let mut tail = self.tail;
        let mut segments = bytes.split(|byte| *byte == b'\n');
        if let Some(first) = segments.next() {
            tail += first.len();
        }
        for segment in segments {
            // A `\n` ended the line that `tail` counts.
            if tail > MAX_LINE_BYTES {
                self.overflowed = true;
                return Err(AiWireError::LineTooLong);
            }
            tail = segment.len();
        }
        if tail > MAX_LINE_BYTES {
            self.overflowed = true;
            return Err(AiWireError::LineTooLong);
        }
        self.tail = tail;
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    /// The next complete, non-blank line (without its `\n` or `\r\n`).
    ///
    /// # Errors
    ///
    /// [`AiWireError::Malformed`] for a line that is not UTF-8 (the line is consumed), and
    /// [`AiWireError::LineTooLong`] after a push failed that way.
    pub fn next_line(&mut self) -> Result<Option<String>, AiWireError> {
        if self.overflowed {
            return Err(AiWireError::LineTooLong);
        }
        loop {
            let unread = self.buffer.get(self.start..).unwrap_or_default();
            let Some(end) = unread.iter().position(|byte| *byte == b'\n') else {
                return Ok(None);
            };
            let raw = unread.get(..end).unwrap_or_default();
            let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
            let decoded = if raw.iter().all(u8::is_ascii_whitespace) {
                None
            } else {
                Some(String::from_utf8(raw.to_vec()).map_err(|_| AiWireError::Malformed))
            };
            self.start += end + 1;
            self.compact();
            match decoded {
                None => {}
                Some(line) => return line.map(Some),
            }
        }
    }

    /// The last line when the stream ended without a newline. Call it after [`next_line`]
    /// returned `None`.
    ///
    /// [`next_line`]: NdjsonDecoder::next_line
    ///
    /// # Errors
    ///
    /// [`AiWireError::Malformed`] for a line that is not UTF-8 (a character cut short by the end
    /// of the stream included) or when complete lines were left unread, and
    /// [`AiWireError::LineTooLong`] after a push failed that way.
    pub fn finish(self) -> Result<Option<String>, AiWireError> {
        if self.overflowed {
            return Err(AiWireError::LineTooLong);
        }
        let rest = self.buffer.get(self.start..).unwrap_or_default();
        if rest.contains(&b'\n') {
            return Err(AiWireError::Malformed);
        }
        let rest = rest.strip_suffix(b"\r").unwrap_or(rest);
        if rest.iter().all(u8::is_ascii_whitespace) {
            return Ok(None);
        }
        String::from_utf8(rest.to_vec())
            .map(Some)
            .map_err(|_| AiWireError::Malformed)
    }

    /// Drops what was read once it is worth the move.
    fn compact(&mut self) {
        if self.start == self.buffer.len() {
            self.buffer.clear();
            self.start = 0;
        } else if self.start >= 64 * 1024 {
            self.buffer.drain(..self.start);
            self.start = 0;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lines_of(chunks: &[&[u8]]) -> Vec<String> {
        let mut decoder = NdjsonDecoder::new();
        let mut out = Vec::new();
        for chunk in chunks {
            decoder.push(chunk).unwrap();
            while let Some(line) = decoder.next_line().unwrap() {
                out.push(line);
            }
        }
        if let Some(last) = decoder.finish().unwrap() {
            out.push(last);
        }
        out
    }

    // ----- decoder -----

    #[test]
    fn complete_lines_come_out_in_order() {
        assert_eq!(lines_of(&[b"a\nb\nc\n"]), vec!["a", "b", "c"]);
    }

    #[test]
    fn a_line_split_across_pushes_is_joined() {
        assert_eq!(
            lines_of(&[b"{\"a\"", b":1}\n{\"b\":", b"2}\n"]),
            vec!["{\"a\":1}", "{\"b\":2}"]
        );
    }

    #[test]
    fn a_utf8_character_split_across_pushes_survives() {
        let line = "{\"t\":\"é 日 😀\"}\n";
        let bytes = line.as_bytes();
        // One byte per push splits every multi-byte character.
        let pieces: Vec<&[u8]> = bytes.chunks(1).collect();
        assert_eq!(lines_of(&pieces), vec!["{\"t\":\"é 日 😀\"}"]);
        // And a split in the middle of the emoji.
        let at = line.find('😀').unwrap() + 2;
        assert_eq!(
            lines_of(&[&bytes[..at], &bytes[at..]]),
            vec!["{\"t\":\"é 日 😀\"}"]
        );
    }

    #[test]
    fn crlf_is_stripped_and_blank_lines_are_skipped() {
        assert_eq!(
            lines_of(&[b"one\r\n\r\n\ntwo\r\n  \r\nthree\n"]),
            vec!["one", "two", "three"]
        );
    }

    #[test]
    fn a_lone_cr_before_the_newline_split_across_pushes_is_stripped() {
        assert_eq!(lines_of(&[b"one\r", b"\ntwo\n"]), vec!["one", "two"]);
    }

    #[test]
    fn finish_returns_a_last_line_without_a_newline() {
        assert_eq!(lines_of(&[b"a\nb"]), vec!["a", "b"]);
        assert_eq!(lines_of(&[b"a\nb\r"]), vec!["a", "b"]);
        assert_eq!(lines_of(&[b"a\n   "]), vec!["a"]);
        assert_eq!(lines_of(&[b""]), Vec::<String>::new());
    }

    #[test]
    fn next_line_waits_for_the_newline() {
        let mut decoder = NdjsonDecoder::new();
        decoder.push(b"partial").unwrap();
        assert_eq!(decoder.next_line().unwrap(), None);
        decoder.push(b" line\n").unwrap();
        assert_eq!(
            decoder.next_line().unwrap().as_deref(),
            Some("partial line")
        );
        assert_eq!(decoder.next_line().unwrap(), None);
    }

    #[test]
    fn a_line_that_is_not_utf8_is_malformed_and_consumed() {
        let mut decoder = NdjsonDecoder::new();
        decoder.push(b"\xff\xfe\nok\n").unwrap();
        assert_eq!(decoder.next_line(), Err(AiWireError::Malformed));
        assert_eq!(decoder.next_line().unwrap().as_deref(), Some("ok"));
    }

    #[test]
    fn a_character_cut_short_by_the_end_of_the_stream_is_malformed() {
        let mut decoder = NdjsonDecoder::new();
        decoder.push(&"é".as_bytes()[..1]).unwrap();
        assert_eq!(decoder.next_line().unwrap(), None);
        assert_eq!(decoder.finish(), Err(AiWireError::Malformed));
    }

    #[test]
    fn a_line_over_one_mebibyte_fails_even_without_a_newline() {
        let mut decoder = NdjsonDecoder::new();
        let big = vec![b'x'; MAX_LINE_BYTES];
        decoder.push(&big).unwrap(); // exactly the limit is fine
        assert_eq!(decoder.push(b"x"), Err(AiWireError::LineTooLong));
        // The failure is permanent.
        assert_eq!(decoder.push(b"\n"), Err(AiWireError::LineTooLong));
        assert_eq!(decoder.next_line(), Err(AiWireError::LineTooLong));
        assert_eq!(decoder.finish(), Err(AiWireError::LineTooLong));
    }

    #[test]
    fn a_long_line_arriving_in_small_pushes_fails_at_the_limit() {
        let mut decoder = NdjsonDecoder::new();
        let piece = vec![b'y'; 4096];
        let mut pushes = 0;
        let result = loop {
            pushes += 1;
            if let Err(error) = decoder.push(&piece) {
                break error;
            }
            assert!(pushes < 1000, "never failed");
        };
        assert_eq!(result, AiWireError::LineTooLong);
        assert_eq!(pushes, MAX_LINE_BYTES / 4096 + 1);
    }

    #[test]
    fn a_finished_line_over_the_limit_fails_inside_one_push() {
        let mut decoder = NdjsonDecoder::new();
        let mut data = vec![b'z'; MAX_LINE_BYTES + 1];
        data.push(b'\n');
        data.extend_from_slice(b"ok\n");
        assert_eq!(decoder.push(&data), Err(AiWireError::LineTooLong));
    }

    #[test]
    fn many_short_lines_never_trip_the_limit_and_the_buffer_is_reclaimed() {
        let mut decoder = NdjsonDecoder::new();
        let line = format!("{}\n", "w".repeat(1000));
        let mut total = 0;
        for _ in 0..3000 {
            decoder.push(line.as_bytes()).unwrap();
            while decoder.next_line().unwrap().is_some() {
                total += 1;
            }
        }
        assert_eq!(total, 3000);
        assert!(decoder.buffer.is_empty());
        assert_eq!(decoder.finish().unwrap(), None);
    }

    #[test]
    fn unread_lines_at_finish_are_reported() {
        let mut decoder = NdjsonDecoder::new();
        decoder.push(b"one\ntwo").unwrap();
        assert_eq!(decoder.finish(), Err(AiWireError::Malformed));
    }

    // ----- chat chunks -----

    #[test]
    fn a_content_chunk_is_parsed() {
        let line = r#"{"model":"qwen3:8b","created_at":"2026-10-13T10:00:00Z","message":{"role":"assistant","content":"Hel"},"done":false}"#;
        let chunk = parse_chat_chunk(line).unwrap();
        assert_eq!(chunk.content, "Hel");
        assert_eq!(chunk.thinking, "");
        assert!(!chunk.done);
        assert_eq!(chunk.done_reason, None);
    }

    #[test]
    fn a_thinking_chunk_is_parsed() {
        let line = r#"{"message":{"role":"assistant","content":"","thinking":"hmm"},"done":false}"#;
        let chunk = parse_chat_chunk(line).unwrap();
        assert_eq!(chunk.content, "");
        assert_eq!(chunk.thinking, "hmm");
    }

    #[test]
    fn the_last_chunk_carries_done_and_the_reason() {
        let line = r#"{"model":"m","message":{"role":"assistant","content":""},"done":true,"done_reason":"length","total_duration":123,"eval_count":7}"#;
        let chunk = parse_chat_chunk(line).unwrap();
        assert!(chunk.done);
        assert_eq!(chunk.done_reason.as_deref(), Some("length"));
    }

    #[test]
    fn a_non_streamed_answer_is_one_done_chunk() {
        let body = r#"{"model":"r2t2","message":{"role":"assistant","content":"language English<asr_text>Hi."},"done":true,"done_reason":"stop"}"#;
        let chunk = parse_chat_chunk(body).unwrap();
        assert_eq!(chunk.content, "language English<asr_text>Hi.");
        assert!(chunk.done);
    }

    #[test]
    fn null_fields_and_extra_fields_are_tolerated() {
        let line = r#"{"message":{"content":null,"thinking":null,"tool_calls":[]},"done":null,"extra":{"a":1}}"#;
        let chunk = parse_chat_chunk(line).unwrap();
        assert_eq!(chunk, ChatChunk::default());
    }

    #[test]
    fn an_error_line_is_remote() {
        let error = parse_chat_chunk(r#"{"error":"model \"x\" not found, try pulling it first"}"#)
            .unwrap_err();
        assert_eq!(
            error,
            AiWireError::Remote("model \"x\" not found, try pulling it first".into())
        );
    }

    #[test]
    fn an_error_wins_over_other_members() {
        let error = parse_chat_chunk(r#"{"message":{"content":"x"},"done":true,"error":"boom"}"#)
            .unwrap_err();
        assert_eq!(error, AiWireError::Remote("boom".into()));
    }

    #[test]
    fn a_long_error_is_cut_to_300_characters() {
        let text = "é".repeat(500);
        let line = json!({ "error": text }).to_string();
        match parse_chat_chunk(&line) {
            Err(AiWireError::Remote(message)) => {
                assert_eq!(message.chars().count(), 300);
                assert!(message.chars().all(|c| c == 'é'));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn odd_errors_still_produce_a_message() {
        assert_eq!(
            parse_chat_chunk(r#"{"error":""}"#),
            Err(AiWireError::Remote("unknown error".into()))
        );
        match parse_chat_chunk(r#"{"error":{"code":500}}"#) {
            Err(AiWireError::Remote(message)) => assert!(message.contains("500")),
            other => panic!("{other:?}"),
        }
        // A null error is not an error.
        assert!(parse_chat_chunk(r#"{"error":null,"done":true}"#).is_ok());
    }

    #[test]
    fn lines_that_are_not_chat_chunks_are_malformed() {
        for line in [
            "",
            "not json",
            "[]",
            "[1,2]",
            "\"text\"",
            "42",
            "null",
            "{}",
            r#"{"model":"m"}"#,
            r#"{"message":"text"}"#,
            r#"{"message":{"content":5}}"#,
            r#"{"done":"yes"}"#,
            "{",
        ] {
            assert_eq!(
                parse_chat_chunk(line),
                Err(AiWireError::Malformed),
                "{line:?}"
            );
        }
    }

    // ----- embed -----

    #[test]
    fn embeddings_come_back_in_order() {
        let body = r#"{"model":"nomic-embed-text","embeddings":[[0.1,0.2,0.3],[1,0,-1]],"total_duration":5,"load_duration":1,"prompt_eval_count":8}"#;
        let vectors = parse_embed(body).unwrap();
        assert_eq!(vectors, vec![vec![0.1, 0.2, 0.3], vec![1.0, 0.0, -1.0]]);
    }

    #[test]
    fn an_empty_embedding_list_is_fine_for_an_empty_input() {
        assert_eq!(
            parse_embed(r#"{"embeddings":[]}"#).unwrap(),
            Vec::<Vec<f32>>::new()
        );
    }

    #[test]
    fn malformed_embeddings_are_refused() {
        for body in [
            "",
            "{}",
            "[]",
            r#"{"embeddings":null}"#,
            r#"{"embeddings":[[]]}"#,
            r#"{"embeddings":[[1,2],[1]]}"#,
            r#"{"embeddings":[["a"]]}"#,
            r#"{"embeddings":"x"}"#,
            r#"{"embedding":[1,2]}"#,
        ] {
            assert_eq!(parse_embed(body), Err(AiWireError::Malformed), "{body:?}");
        }
    }

    #[test]
    fn an_embed_error_is_remote() {
        assert_eq!(
            parse_embed(r#"{"error":"the input length exceeds the context length"}"#),
            Err(AiWireError::Remote(
                "the input length exceeds the context length".into()
            ))
        );
    }

    // ----- tags -----

    #[test]
    fn tags_of_ollama_0_35_are_parsed() {
        let body = r#"{"models":[
            {"name":"qwen3:8b","model":"qwen3:8b","modified_at":"2026-09-01T10:00:00Z","size":5225388164,
             "digest":"abc","details":{"parent_model":"","format":"gguf","family":"qwen3","families":["qwen3"],
             "parameter_size":"8.2B","quantization_level":"Q4_K_M"},
             "capabilities":["completion","tools","thinking"]},
            {"name":"nomic-embed-text:latest","model":"nomic-embed-text:latest","size":274302450,
             "details":{"family":"nomic-bert","parameter_size":"137M"},"capabilities":["embedding"]},
            {"name":"r2t2:latest","model":"r2t2:latest","size":9000000000,
             "details":{"family":"qwen3asr","parameter_size":"1.7B"},"capabilities":["completion","audio"]}
        ]}"#;
        let models = parse_tags(body).unwrap();
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].name, "qwen3:8b");
        assert_eq!(models[0].family.as_deref(), Some("qwen3"));
        assert_eq!(models[0].parameter_size.as_deref(), Some("8.2B"));
        assert_eq!(models[0].size, 5_225_388_164);
        assert_eq!(
            models[0].capabilities,
            vec!["completion", "tools", "thinking"]
        );
        assert_eq!(models[1].capabilities, vec!["embedding"]);
        assert_eq!(models[2].capabilities, vec!["completion", "audio"]);
    }

    #[test]
    fn missing_capabilities_are_derived_from_the_name() {
        let body = r#"{"models":[
            {"name":"mxbai-embed-large:latest","size":1},
            {"name":"Snowflake-Arctic-EMBED:335m","size":2,"capabilities":[]},
            {"name":"llama3.2:3b","size":3},
            {"name":"gemma4:12b","size":4,"capabilities":["completion","vision"]}
        ]}"#;
        let models = parse_tags(body).unwrap();
        assert_eq!(models[0].capabilities, vec!["embedding"]);
        assert_eq!(models[1].capabilities, vec!["embedding"]);
        assert_eq!(models[2].capabilities, vec!["completion"]);
        assert_eq!(models[3].capabilities, vec!["completion", "vision"]);
        assert_eq!(models[0].family, None);
        assert_eq!(models[0].parameter_size, None);
    }

    #[test]
    fn tags_tolerate_missing_optional_fields_and_use_model_when_name_is_missing() {
        let body = r#"{"models":[{"model":"only-model:1"},{"name":"","model":""},{}, {"name":"n","details":{"family":"","parameter_size":null}}]}"#;
        let models = parse_tags(body).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].name, "only-model:1");
        assert_eq!(models[0].size, 0);
        assert_eq!(models[1].name, "n");
        assert_eq!(models[1].family, None);
        assert_eq!(models[1].parameter_size, None);
    }

    #[test]
    fn no_models_is_an_empty_list() {
        assert!(parse_tags(r#"{"models":[]}"#).unwrap().is_empty());
    }

    #[test]
    fn bad_tags_are_refused() {
        for body in [
            "",
            "[]",
            "{}",
            r#"{"models":null}"#,
            r#"{"models":"x"}"#,
            "nope",
        ] {
            assert_eq!(parse_tags(body), Err(AiWireError::Malformed), "{body:?}");
        }
        assert_eq!(
            parse_tags(r#"{"error":"unauthorized"}"#),
            Err(AiWireError::Remote("unauthorized".into()))
        );
    }

    // ----- show -----

    #[test]
    fn the_context_length_is_read_from_model_info() {
        let body = r#"{"license":"x","details":{"family":"qwen3"},
            "model_info":{"general.architecture":"qwen3","general.parameter_count":8190735360,
            "qwen3.block_count":36,"qwen3.context_length":40960,"qwen3.embedding_length":4096}}"#;
        assert_eq!(context_length_of_show(body), Some(40960));
    }

    #[test]
    fn the_architecture_key_wins_over_other_context_lengths() {
        let body = r#"{"model_info":{"general.architecture":"gemma3",
            "clip.context_length":77,"gemma3.context_length":131072}}"#;
        assert_eq!(context_length_of_show(body), Some(131_072));
    }

    #[test]
    fn any_key_ending_in_context_length_is_a_fallback() {
        let body = r#"{"model_info":{"nomic-bert.context_length":2048}}"#;
        assert_eq!(context_length_of_show(body), Some(2048));
    }

    #[test]
    fn a_missing_or_odd_context_length_is_none() {
        for body in [
            "",
            "{}",
            "nope",
            r#"{"model_info":null}"#,
            r#"{"model_info":{}}"#,
            r#"{"model_info":{"general.architecture":"x","x.embedding_length":4096}}"#,
            r#"{"model_info":{"x.context_length":"long"}}"#,
            r#"{"model_info":{"x.context_length":0}}"#,
            r#"{"model_info":{"x.context_length":-5}}"#,
            r#"{"model_info":{"x.context_length":99999999999}}"#,
        ] {
            assert_eq!(context_length_of_show(body), None, "{body:?}");
        }
    }

    // ----- requests -----

    #[test]
    fn a_chat_request_serialises_to_ollamas_shape() {
        let request = ChatRequest {
            model: "qwen3:8b".into(),
            messages: vec![
                WireMessage::text("system", "s"),
                WireMessage {
                    role: "user".into(),
                    content: "u".into(),
                    images: vec!["AAAA".into()],
                },
            ],
            stream: true,
            think: None,
            options: ChatOptions {
                temperature: Some(0.5),
                num_ctx: Some(4096),
                num_predict: None,
            },
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(
            value,
            json!({
                "model": "qwen3:8b",
                "messages": [
                    { "role": "system", "content": "s" },
                    { "role": "user", "content": "u", "images": ["AAAA"] }
                ],
                "stream": true,
                "options": { "temperature": 0.5, "num_ctx": 4096 }
            })
        );
        let with_think = ChatRequest {
            think: Some(false),
            ..request
        };
        assert_eq!(
            serde_json::to_value(&with_think).unwrap()["think"],
            json!(false)
        );
    }

    #[test]
    fn an_embed_request_serialises_to_ollamas_shape() {
        let request = EmbedRequest {
            model: "m".into(),
            input: vec!["a".into(), "b".into()],
            truncate: true,
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({ "model": "m", "input": ["a", "b"], "truncate": true })
        );
    }

    #[test]
    fn the_version_answer_is_parsed() {
        let version: VersionResponse = serde_json::from_str(r#"{"version":"0.35.0"}"#).unwrap();
        assert_eq!(version.version, "0.35.0");
    }
}
