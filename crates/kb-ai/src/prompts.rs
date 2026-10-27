//! The prompts (`docs/architecture/ai.md`, section 3.2): the writing actions, the chat over
//! retrieved sources, the transcription and the parts of a long meeting summary.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use kb_types::ai::{AiAction, AiChatInput, AiGenerateInput, AiRole, AiSource, AiTone};

use crate::ollama::{ChatOptions, ChatRequest, WireMessage};
use crate::util::{byte_index_of_char, char_len, longer_than, truncate_chars};

/// Most characters of a writing input's `text`.
pub const MAX_TEXT_CHARS: usize = 32_000;
/// Most characters of a writing input's `context`.
pub const MAX_CONTEXT_CHARS: usize = 8_000;
/// Most characters of a custom instruction.
pub const MAX_PROMPT_CHARS: usize = 2_000;
/// Most characters of a target language name.
pub const MAX_LANGUAGE_CHARS: usize = 60;
/// Most messages of a chat.
pub const MAX_CHAT_MESSAGES: usize = 40;
/// Most characters of one chat message.
pub const MAX_MESSAGE_CHARS: usize = 16_000;
/// Most characters of a whole chat.
pub const MAX_CHAT_CHARS: usize = 64_000;
/// A transcript longer than this is summarised in parts.
pub const SUMMARY_PART_CHARS: usize = 24_000;
/// Most characters of the question embedded for retrieval (the core's limit on a query).
pub const MAX_RETRIEVAL_QUERY_CHARS: usize = 2_000;

/// The smallest context window asked for (tokens).
pub const NUM_CTX_MIN: u32 = 4_096;
/// The largest context window asked for (tokens), and the one assumed for a model of unknown
/// length.
pub const NUM_CTX_CAP: u32 = 16_384;

/// System prompt of every writing action.
pub const WRITING_SYSTEM_PROMPT: &str = "You are a writing assistant inside a notes app. Reply with the result only, in Markdown, without preamble, without explanations, without code fences around the whole answer. Keep the language of the input unless asked to translate.";

/// First part of the system prompt of the chat (the numbered sources follow).
pub const CHAT_SYSTEM_PROMPT: &str = "You answer questions about the user's notes. Use only the numbered sources below. Cite them inline as [1], [2]. If the sources do not contain the answer, say so plainly. Answer in the language of the question.";

/// The user message of a transcription (the audio travels in `images`).
pub const TRANSCRIPTION_PROMPT: &str =
    "Transcribe the speech in this audio clip verbatim. Output only the transcript.";

/// The instruction that merges the part summaries of a long meeting (used as a `custom` action).
pub const MERGE_SUMMARIES_PROMPT: &str =
    "Merge these partial meeting summaries into one with the same three sections.";

/// Characters kept of an earlier question, at least, for it to be worth prefixing a retrieval
/// query with.
const MIN_PREVIOUS_QUERY_CHARS: usize = 16;

/// What the shell knows of a model from Ollama.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelTraits {
    /// Has the `thinking` capability (only then is `think` sent).
    pub thinking: bool,
    /// Its context length (tokens); 16,384 when unknown.
    pub num_ctx_max: u32,
}

/// A writing or chat input that breaks a limit; `field` is the API field to report.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {message}")]
pub struct AiInputError {
    /// Field (`text`, `context`, `prompt`, `language`, `tone`, `messages`).
    pub field: &'static str,
    /// Short reason.
    pub message: String,
}

impl AiInputError {
    fn new(field: &'static str, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }
}

/// The streamed chat request of a writing action.
///
/// The system message is [`WRITING_SYSTEM_PROMPT`]. The user message is the action's
/// instruction, then the page context (when there is one) in a `<context>` block, then the text
/// (when there is one) in a `<text>` block. Temperature per action as in the design; `think` is
/// `Some(think)` only when `traits.thinking`; `options.num_ctx` is [`num_ctx`] of the whole
/// prompt.
///
/// The text may be empty for `continue` (when there is a context) and `custom`; every other
/// action needs some. `tone`, `language` (at most 60 characters, one line) and `prompt` are
/// needed by the action that uses them.
///
/// # Errors
///
/// [`AiInputError`] for a limit or a missing `text`, `tone`, `language` or `prompt`.
pub fn build_generate(
    model: &str,
    traits: ModelTraits,
    think: bool,
    input: &AiGenerateInput,
) -> Result<ChatRequest, AiInputError> {
    if longer_than(&input.text, MAX_TEXT_CHARS) {
        return Err(AiInputError::new(
            "text",
            format!("longer than {MAX_TEXT_CHARS} characters"),
        ));
    }
    if input
        .context
        .as_deref()
        .is_some_and(|context| longer_than(context, MAX_CONTEXT_CHARS))
    {
        return Err(AiInputError::new(
            "context",
            format!("longer than {MAX_CONTEXT_CHARS} characters"),
        ));
    }
    let context = input
        .context
        .as_deref()
        .map(str::trim)
        .filter(|context| !context.is_empty());
    let (instruction, temperature) = instruction_of(input, context.is_some())?;
    let user = user_message(&instruction, context, &input.text);
    let total = char_len(WRITING_SYSTEM_PROMPT) + char_len(&user);
    Ok(ChatRequest {
        model: model.to_owned(),
        messages: vec![
            WireMessage::text("system", WRITING_SYSTEM_PROMPT),
            WireMessage::text("user", user),
        ],
        stream: true,
        think: think_for(traits, think),
        options: ChatOptions {
            temperature: Some(temperature),
            num_ctx: Some(num_ctx(total, traits.num_ctx_max)),
            num_predict: None,
        },
    })
}

/// `Some(think)` for a model that can think, `None` for any other (Ollama refuses `think` there).
fn think_for(traits: ModelTraits, think: bool) -> Option<bool> {
    traits.thinking.then_some(think)
}

/// The instruction and the temperature of an action, after checking what the action needs.
fn instruction_of(
    input: &AiGenerateInput,
    has_context: bool,
) -> Result<(String, f32), AiInputError> {
    let blank_text = input.text.trim().is_empty();
    let needs_text = || {
        if blank_text {
            Err(AiInputError::new("text", "there is no text to work on"))
        } else {
            Ok(())
        }
    };
    let fixed = |instruction: &str, temperature: f32| -> Result<(String, f32), AiInputError> {
        needs_text()?;
        Ok((instruction.to_owned(), temperature))
    };
    match input.action {
        AiAction::Continue => {
            if blank_text && !has_context {
                return Err(AiInputError::new("text", "there is nothing to continue"));
            }
            Ok((
                "Continue the text naturally for one or two paragraphs. Context precedes. Do not repeat it."
                    .to_owned(),
                0.7,
            ))
        }
        AiAction::Improve => fixed(
            "Improve the writing: clarity, flow, word choice. Keep the meaning and the formatting.",
            0.4,
        ),
        AiAction::FixGrammar => fixed(
            "Fix spelling, grammar and punctuation only. Change nothing else.",
            0.1,
        ),
        AiAction::Shorter => fixed("Make it about half as long, keeping the key points.", 0.3),
        AiAction::Longer => fixed(
            "Expand it to about twice the length with relevant detail. Invent no facts.",
            0.6,
        ),
        AiAction::Simplify => fixed("Rewrite in plain, simple language.", 0.3),
        AiAction::Tone => {
            needs_text()?;
            let tone = input
                .tone
                .ok_or_else(|| AiInputError::new("tone", "a tone is needed"))?;
            Ok((format!("Rewrite in a {} tone.", tone_word(tone)), 0.4))
        }
        AiAction::Translate => {
            needs_text()?;
            let language = input.language.as_deref().map(str::trim).unwrap_or_default();
            if language.is_empty() {
                return Err(AiInputError::new("language", "a language is needed"));
            }
            if longer_than(language, MAX_LANGUAGE_CHARS) {
                return Err(AiInputError::new(
                    "language",
                    format!("longer than {MAX_LANGUAGE_CHARS} characters"),
                ));
            }
            if language.contains(['\n', '\r']) {
                return Err(AiInputError::new("language", "must be one line"));
            }
            Ok((format!("Translate into {language}."), 0.2))
        }
        AiAction::Summarize => fixed(
            "Summarise as a short paragraph followed by key points as a bulleted list.",
            0.2,
        ),
        AiAction::Explain => fixed("Explain what this means in simple terms.", 0.3),
        AiAction::ActionItems => fixed(
            "List the action items as a Markdown task list (`- [ ] ...`), with owners and dates when stated. Write \"No action items.\" if none.",
            0.1,
        ),
        AiAction::Custom => {
            let prompt = input.prompt.as_deref().map(str::trim).unwrap_or_default();
            if prompt.is_empty() {
                return Err(AiInputError::new("prompt", "an instruction is needed"));
            }
            if longer_than(prompt, MAX_PROMPT_CHARS) {
                return Err(AiInputError::new(
                    "prompt",
                    format!("longer than {MAX_PROMPT_CHARS} characters"),
                ));
            }
            Ok((prompt.to_owned(), 0.6))
        }
        AiAction::MeetingSummary => fixed(
            "From this meeting transcript write `### Summary` (2 to 4 sentences), `### Key points` (bullets), `### Action items` (a Markdown task list: one `- [ ] ` line per item, with the owner and the date when stated; write \"None.\" when there are none).",
            0.2,
        ),
    }
}

/// The word of a tone in the instruction.
fn tone_word(tone: AiTone) -> &'static str {
    match tone {
        AiTone::Professional => "professional",
        AiTone::Casual => "casual",
        AiTone::Friendly => "friendly",
        AiTone::Confident => "confident",
        AiTone::Direct => "direct",
    }
}

/// The user message: the instruction, the context block (before the text) and the text block.
fn user_message(instruction: &str, context: Option<&str>, text: &str) -> String {
    let mut out = String::from(instruction);
    if let Some(context) = context {
        out.push_str(
            "\n\nContext (the page title and the text before; for reference only):\n<context>\n",
        );
        out.push_str(context);
        out.push_str("\n</context>");
    }
    if !text.trim().is_empty() {
        out.push_str("\n\nText:\n<text>\n");
        out.push_str(text);
        out.push_str("\n</text>");
    }
    out
}

/// The streamed chat request of a question over numbered sources (the system prompt lists them).
///
/// The system message is [`CHAT_SYSTEM_PROMPT`] followed by the sources as `[n] <title>` and the
/// passage (numbered by their own `n`). The conversation follows. When it is over
/// [`MAX_CHAT_CHARS`] characters, or would not leave room for an answer in the model's context
/// (about three characters per token, 1,024 tokens kept for the answer), the oldest messages are
/// dropped first (and then any assistant message left first); the last message, the question,
/// is always kept. Temperature 0.3, `think` as for [`build_generate`].
///
/// # Errors
///
/// [`AiInputError`] for the message limits or a conversation that does not end with the person.
pub fn build_chat(
    model: &str,
    traits: ModelTraits,
    think: bool,
    input: &AiChatInput,
    sources: &[AiSource],
) -> Result<ChatRequest, AiInputError> {
    let messages = &input.messages;
    if messages.is_empty() {
        return Err(AiInputError::new("messages", "there is no question"));
    }
    if messages.len() > MAX_CHAT_MESSAGES {
        return Err(AiInputError::new(
            "messages",
            format!("more than {MAX_CHAT_MESSAGES} messages"),
        ));
    }
    if messages
        .iter()
        .any(|message| longer_than(&message.content, MAX_MESSAGE_CHARS))
    {
        return Err(AiInputError::new(
            "messages",
            format!("a message is longer than {MAX_MESSAGE_CHARS} characters"),
        ));
    }
    let Some(last) = messages.last() else {
        return Err(AiInputError::new("messages", "there is no question"));
    };
    if last.role != AiRole::User {
        return Err(AiInputError::new(
            "messages",
            "the conversation must end with a question from the person",
        ));
    }
    if last.content.trim().is_empty() {
        return Err(AiInputError::new("messages", "the question is empty"));
    }

    let system = chat_system_prompt(sources);
    let system_chars = char_len(&system);
    let lengths: Vec<usize> = messages
        .iter()
        .map(|message| char_len(&message.content))
        .collect();
    let context_chars = usize::try_from(context_cap(traits.num_ctx_max).saturating_sub(1_024))
        .unwrap_or(0)
        .saturating_mul(3);
    let budget = MAX_CHAT_CHARS.min(context_chars.saturating_sub(system_chars));
    let mut total: usize = lengths.iter().sum();
    let mut first = 0;
    while first + 1 < messages.len() && total > budget {
        total -= lengths.get(first).copied().unwrap_or(0);
        first += 1;
    }
    if first > 0 {
        // Start on a question, not on half an exchange.
        while first + 1 < messages.len()
            && messages
                .get(first)
                .is_some_and(|m| m.role == AiRole::Assistant)
        {
            total -= lengths.get(first).copied().unwrap_or(0);
            first += 1;
        }
    }

    let mut wire = vec![WireMessage::text("system", system)];
    for message in messages.iter().skip(first) {
        let role = match message.role {
            AiRole::User => "user",
            AiRole::Assistant => "assistant",
        };
        wire.push(WireMessage::text(role, message.content.as_str()));
    }
    Ok(ChatRequest {
        model: model.to_owned(),
        messages: wire,
        stream: true,
        think: think_for(traits, think),
        options: ChatOptions {
            temperature: Some(0.3),
            num_ctx: Some(num_ctx(system_chars + total, traits.num_ctx_max)),
            num_predict: None,
        },
    })
}

/// The system prompt of the chat: the instructions, then every source as `[n] title` and text.
fn chat_system_prompt(sources: &[AiSource]) -> String {
    let mut out = String::from(CHAT_SYSTEM_PROMPT);
    out.push_str("\n\nSources:");
    if sources.is_empty() {
        out.push_str("\n(none found)");
    }
    for source in sources {
        let title = source.title.trim();
        let title = if title.is_empty() { "Untitled" } else { title };
        out.push_str(&format!("\n\n[{}] {}\n{}", source.n, title, source.text));
    }
    out
}

/// The non-streamed chat request that transcribes one WAV segment (base64 in `images`).
///
/// Temperature 0 and nothing else set: the model's own context window applies. No `think`.
#[must_use]
pub fn build_transcription(model: &str, wav: &[u8]) -> ChatRequest {
    ChatRequest {
        model: model.to_owned(),
        messages: vec![WireMessage {
            role: "user".to_owned(),
            content: TRANSCRIPTION_PROMPT.to_owned(),
            images: vec![STANDARD.encode(wav)],
        }],
        stream: false,
        think: None,
        options: ChatOptions {
            temperature: Some(0.0),
            num_ctx: None,
            num_predict: None,
        },
    }
}

/// A long transcript cut at paragraph boundaries (blank lines) into parts of at most
/// [`SUMMARY_PART_CHARS`] characters; a transcript that is not longer is one part, unchanged.
///
/// Paragraphs are packed greedily and re-joined with a blank line. A paragraph over the limit
/// is cut at its last line break, else its last space, else at the limit.
#[must_use]
pub fn summary_parts(transcript: &str) -> Vec<String> {
    if !longer_than(transcript, SUMMARY_PART_CHARS) {
        return vec![transcript.to_owned()];
    }
    let mut units: Vec<&str> = Vec::new();
    for paragraph in transcript.split("\n\n") {
        let paragraph = paragraph.trim();
        if paragraph.is_empty() {
            continue;
        }
        if longer_than(paragraph, SUMMARY_PART_CHARS) {
            split_oversized(paragraph, &mut units);
        } else {
            units.push(paragraph);
        }
    }
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0usize;
    for unit in units {
        let length = char_len(unit);
        if current_chars > 0 && current_chars + 2 + length > SUMMARY_PART_CHARS {
            parts.push(std::mem::take(&mut current));
            current_chars = 0;
        }
        if current_chars > 0 {
            current.push_str("\n\n");
            current_chars += 2;
        }
        current.push_str(unit);
        current_chars += length;
    }
    if current_chars > 0 {
        parts.push(current);
    }
    parts
}

/// Cuts a paragraph of more than [`SUMMARY_PART_CHARS`] characters into pieces that fit.
fn split_oversized<'a>(paragraph: &'a str, out: &mut Vec<&'a str>) {
    let mut rest = paragraph.trim();
    while longer_than(rest, SUMMARY_PART_CHARS) {
        let window_end = byte_index_of_char(rest, SUMMARY_PART_CHARS);
        let window = rest.get(..window_end).unwrap_or(rest);
        let cut = window
            .rfind('\n')
            .or_else(|| window.rfind(' '))
            .filter(|index| *index > 0)
            .unwrap_or(window_end);
        let head = rest.get(..cut).unwrap_or(rest).trim();
        if !head.is_empty() {
            out.push(head);
        }
        rest = rest.get(cut..).unwrap_or("").trim_start();
    }
    if !rest.is_empty() {
        out.push(rest);
    }
}

/// The question to embed for retrieval: the last user message, prefixed by the previous user
/// message (and a new line) when there is one, for follow-ups. Cut to
/// [`MAX_RETRIEVAL_QUERY_CHARS`] characters: the last message first, the previous one only for
/// what room is left (and only when that is a useful amount). Empty when nobody wrote.
#[must_use]
pub fn retrieval_query(input: &AiChatInput) -> String {
    let mut questions = input
        .messages
        .iter()
        .rev()
        .filter(|message| message.role == AiRole::User)
        .map(|message| message.content.trim())
        .filter(|text| !text.is_empty());
    let Some(last) = questions.next() else {
        return String::new();
    };
    let last = truncate_chars(last, MAX_RETRIEVAL_QUERY_CHARS);
    let Some(previous) = questions.next() else {
        return last.to_owned();
    };
    let room = MAX_RETRIEVAL_QUERY_CHARS.saturating_sub(char_len(last) + 1);
    if room < MIN_PREVIOUS_QUERY_CHARS {
        return last.to_owned();
    }
    let previous = truncate_chars(previous, room).trim_end();
    format!("{previous}\n{last}")
}

/// An answer cleaned for display and insertion: `<think>` blocks (an unterminated one too: all
/// that follows it is thinking) and one code fence that wraps the whole answer (```` ``` ````,
/// ```` ```markdown ```` or ```` ```md ````) removed, trimmed.
///
/// A fence around code of another language is the answer itself and stays, and so do several
/// separate code blocks.
#[must_use]
pub fn clean_output(text: &str) -> String {
    let without_thinking = strip_think(text);
    let trimmed = without_thinking.trim();
    match strip_wrapping_fence(trimmed) {
        Some(inner) => inner.trim().to_owned(),
        None => trimmed.to_owned(),
    }
}

/// `text` without `<think>…</think>` blocks. A closing tag with no opening tag before it ends
/// reasoning that a model wrote without the opening tag (qwen3:4b ignores `think: false` that way):
/// everything up to it goes.
fn strip_think(text: &str) -> String {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    if let Some(close) = rest.find(CLOSE)
        && !rest.get(..close).unwrap_or_default().contains(OPEN)
    {
        rest = rest.get(close + CLOSE.len()..).unwrap_or_default();
    }
    while let Some(start) = rest.find(OPEN) {
        out.push_str(rest.get(..start).unwrap_or_default());
        let after = rest.get(start + OPEN.len()..).unwrap_or_default();
        match after.find(CLOSE) {
            Some(end) => rest = after.get(end + CLOSE.len()..).unwrap_or_default(),
            // Never closed: the rest is the model still thinking (or cut off while it did).
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// The text inside a fence that wraps all of `text` (already trimmed), when the fence is bare or
/// says `markdown` or `md`.
fn strip_wrapping_fence(text: &str) -> Option<&str> {
    let newline = text.find('\n')?;
    let language = text.get(..newline)?.trim().strip_prefix("```")?.trim();
    let bare = language.is_empty();
    if !(bare || language.eq_ignore_ascii_case("markdown") || language.eq_ignore_ascii_case("md")) {
        return None;
    }
    let body = text.get(newline + 1..)?.trim_end();
    let inner = body.strip_suffix("```")?;
    // The closing fence is on its own line.
    let inner = inner.trim_end_matches([' ', '\t']);
    if !(inner.is_empty() || inner.ends_with('\n')) {
        return None;
    }
    let inner_fences = inner
        .lines()
        .filter(|line| line.trim_start().starts_with("```"))
        .count();
    // A bare wrapper holds no other fence (that would be separate blocks); a Markdown wrapper
    // may hold balanced ones.
    let balanced = if bare {
        inner_fences == 0
    } else {
        inner_fences.is_multiple_of(2)
    };
    balanced.then_some(inner)
}

/// The context window to ask for: `clamp(round_up_1024(prompt_chars / 3 + 1024), 4096,
/// min(16384, max))`. A `max` of 0 means unknown (16,384); a `max` under 4,096 wins over the
/// floor, since the model cannot take more.
#[must_use]
pub fn num_ctx(prompt_chars: usize, max: u32) -> u32 {
    let wanted = u64::try_from(prompt_chars).unwrap_or(u64::MAX) / 3 + 1_024;
    let rounded = wanted.div_ceil(1_024).saturating_mul(1_024);
    let rounded = u32::try_from(rounded).unwrap_or(u32::MAX);
    let upper = context_cap(max);
    if rounded < NUM_CTX_MIN {
        NUM_CTX_MIN.min(upper)
    } else {
        rounded.min(upper)
    }
}

/// The largest window to ask a model for: its own length up to [`NUM_CTX_CAP`] (0: unknown).
fn context_cap(max: u32) -> u32 {
    if max == 0 {
        NUM_CTX_CAP
    } else {
        max.min(NUM_CTX_CAP)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use kb_types::ai::{AiChatMessage, AiScope};
    use kb_types::{PageId, SpaceId};

    const THINKER: ModelTraits = ModelTraits {
        thinking: true,
        num_ctx_max: 40_960,
    };
    const PLAIN: ModelTraits = ModelTraits {
        thinking: false,
        num_ctx_max: 16_384,
    };

    fn generate(action: AiAction, text: &str) -> AiGenerateInput {
        AiGenerateInput {
            action,
            text: text.to_owned(),
            context: None,
            language: None,
            tone: None,
            prompt: None,
        }
    }

    fn build(input: &AiGenerateInput) -> ChatRequest {
        build_generate("qwen3:8b", PLAIN, false, input).unwrap()
    }

    fn user_text(request: &ChatRequest) -> &str {
        &request.messages[1].content
    }

    // ----- build_generate -----

    #[test]
    fn every_writing_action_has_the_designed_instruction_and_temperature() {
        let table: [(AiAction, &str, f32); 13] = [
            (
                AiAction::Continue,
                "Continue the text naturally for one or two paragraphs. Context precedes. Do not repeat it.",
                0.7,
            ),
            (
                AiAction::Improve,
                "Improve the writing: clarity, flow, word choice. Keep the meaning and the formatting.",
                0.4,
            ),
            (
                AiAction::FixGrammar,
                "Fix spelling, grammar and punctuation only. Change nothing else.",
                0.1,
            ),
            (
                AiAction::Shorter,
                "Make it about half as long, keeping the key points.",
                0.3,
            ),
            (
                AiAction::Longer,
                "Expand it to about twice the length with relevant detail. Invent no facts.",
                0.6,
            ),
            (
                AiAction::Simplify,
                "Rewrite in plain, simple language.",
                0.3,
            ),
            (AiAction::Tone, "Rewrite in a friendly tone.", 0.4),
            (AiAction::Translate, "Translate into French.", 0.2),
            (
                AiAction::Summarize,
                "Summarise as a short paragraph followed by key points as a bulleted list.",
                0.2,
            ),
            (
                AiAction::Explain,
                "Explain what this means in simple terms.",
                0.3,
            ),
            (
                AiAction::ActionItems,
                "List the action items as a Markdown task list (`- [ ] ...`), with owners and dates when stated. Write \"No action items.\" if none.",
                0.1,
            ),
            (AiAction::Custom, "Make it rhyme.", 0.6),
            (
                AiAction::MeetingSummary,
                "From this meeting transcript write `### Summary` (2 to 4 sentences), `### Key points` (bullets), `### Action items` (a Markdown task list: one `- [ ] ` line per item, with the owner and the date when stated; write \"None.\" when there are none).",
                0.2,
            ),
        ];
        for (action, instruction, temperature) in table {
            let mut input = generate(action, "Some text.");
            input.tone = Some(AiTone::Friendly);
            input.language = Some("French".into());
            input.prompt = Some("Make it rhyme.".into());
            let request = build(&input);
            assert_eq!(request.messages.len(), 2, "{action:?}");
            assert_eq!(request.messages[0].role, "system");
            assert_eq!(request.messages[0].content, WRITING_SYSTEM_PROMPT);
            assert_eq!(request.messages[1].role, "user");
            assert!(
                user_text(&request).starts_with(&format!(
                    "{instruction}\n\nText:\n<text>\nSome text.\n</text>"
                )),
                "{action:?}: {}",
                user_text(&request)
            );
            assert_eq!(request.options.temperature, Some(temperature), "{action:?}");
            assert!(request.stream, "{action:?}");
            assert_eq!(request.model, "qwen3:8b");
            assert_eq!(request.options.num_predict, None);
        }
    }

    #[test]
    fn the_system_prompt_is_the_designed_one() {
        assert_eq!(
            WRITING_SYSTEM_PROMPT,
            "You are a writing assistant inside a notes app. Reply with the result only, in Markdown, without preamble, without explanations, without code fences around the whole answer. Keep the language of the input unless asked to translate."
        );
    }

    #[test]
    fn every_tone_has_its_word() {
        for (tone, word) in [
            (AiTone::Professional, "professional"),
            (AiTone::Casual, "casual"),
            (AiTone::Friendly, "friendly"),
            (AiTone::Confident, "confident"),
            (AiTone::Direct, "direct"),
        ] {
            let mut input = generate(AiAction::Tone, "Hi.");
            input.tone = Some(tone);
            assert!(user_text(&build(&input)).starts_with(&format!("Rewrite in a {word} tone.")));
        }
    }

    #[test]
    fn the_context_is_a_separate_block_before_the_text() {
        let mut input = generate(AiAction::Improve, "the selection");
        input.context = Some("  My Page\nEarlier paragraph.  ".into());
        let request = build(&input);
        let user = user_text(&request);
        let context_at = user
            .find("<context>\nMy Page\nEarlier paragraph.\n</context>")
            .unwrap();
        let text_at = user.find("<text>\nthe selection\n</text>").unwrap();
        assert!(context_at < text_at);
        assert!(user.starts_with("Improve the writing"));
    }

    #[test]
    fn a_blank_context_adds_no_block() {
        let mut input = generate(AiAction::Improve, "x");
        input.context = Some("  \n ".into());
        assert!(!user_text(&build(&input)).contains("<context>"));
        input.context = None;
        assert!(!user_text(&build(&input)).contains("<context>"));
    }

    #[test]
    fn continue_may_have_an_empty_text_when_there_is_a_context() {
        let mut input = generate(AiAction::Continue, "");
        input.context = Some("Title\nOnce upon a time".into());
        let request = build(&input);
        assert!(user_text(&request).contains("<context>\nTitle\nOnce upon a time\n</context>"));
        assert!(!user_text(&request).contains("<text>"));
        // Neither text nor context: nothing to continue.
        let error =
            build_generate("m", PLAIN, false, &generate(AiAction::Continue, " ")).unwrap_err();
        assert_eq!(error.field, "text");
    }

    #[test]
    fn custom_may_be_a_free_request_without_text_or_context() {
        let mut input = generate(AiAction::Custom, "");
        input.prompt = Some("  Write a haiku about autumn.  ".into());
        let request = build(&input);
        assert_eq!(user_text(&request), "Write a haiku about autumn.");
        assert_eq!(request.options.temperature, Some(0.6));
    }

    #[test]
    fn the_other_actions_need_text() {
        for action in [
            AiAction::Improve,
            AiAction::FixGrammar,
            AiAction::Shorter,
            AiAction::Longer,
            AiAction::Simplify,
            AiAction::Tone,
            AiAction::Translate,
            AiAction::Summarize,
            AiAction::Explain,
            AiAction::ActionItems,
            AiAction::MeetingSummary,
        ] {
            let mut input = generate(action, "  \n");
            input.tone = Some(AiTone::Direct);
            input.language = Some("German".into());
            let error = build_generate("m", PLAIN, false, &input).unwrap_err();
            assert_eq!(error.field, "text", "{action:?}");
        }
    }

    #[test]
    fn limits_name_the_field() {
        let mut input = generate(AiAction::Improve, &"x".repeat(MAX_TEXT_CHARS + 1));
        assert_eq!(
            build_generate("m", PLAIN, false, &input).unwrap_err().field,
            "text"
        );
        input.text = "x".repeat(MAX_TEXT_CHARS);
        assert!(build_generate("m", PLAIN, false, &input).is_ok());
        input.context = Some("c".repeat(MAX_CONTEXT_CHARS + 1));
        assert_eq!(
            build_generate("m", PLAIN, false, &input).unwrap_err().field,
            "context"
        );
        input.context = Some("c".repeat(MAX_CONTEXT_CHARS));
        assert!(build_generate("m", PLAIN, false, &input).is_ok());
        // Characters, not bytes.
        input.context = Some("é".repeat(MAX_CONTEXT_CHARS));
        assert!(build_generate("m", PLAIN, false, &input).is_ok());
        input.context = None;
        input.text = "é".repeat(MAX_TEXT_CHARS);
        assert!(build_generate("m", PLAIN, false, &input).is_ok());
    }

    #[test]
    fn a_custom_prompt_is_needed_and_limited() {
        let mut input = generate(AiAction::Custom, "text");
        assert_eq!(
            build_generate("m", PLAIN, false, &input).unwrap_err().field,
            "prompt"
        );
        input.prompt = Some("   ".into());
        assert_eq!(
            build_generate("m", PLAIN, false, &input).unwrap_err().field,
            "prompt"
        );
        input.prompt = Some("p".repeat(MAX_PROMPT_CHARS + 1));
        assert_eq!(
            build_generate("m", PLAIN, false, &input).unwrap_err().field,
            "prompt"
        );
        input.prompt = Some("p".repeat(MAX_PROMPT_CHARS));
        assert!(build_generate("m", PLAIN, false, &input).is_ok());
    }

    #[test]
    fn a_prompt_is_ignored_by_other_actions_whatever_its_length() {
        let mut input = generate(AiAction::Improve, "text");
        input.prompt = Some("p".repeat(MAX_PROMPT_CHARS * 2));
        assert!(build_generate("m", PLAIN, false, &input).is_ok());
    }

    #[test]
    fn tone_needs_a_tone() {
        let input = generate(AiAction::Tone, "text");
        assert_eq!(
            build_generate("m", PLAIN, false, &input).unwrap_err().field,
            "tone"
        );
    }

    #[test]
    fn translate_needs_a_short_one_line_language() {
        let mut input = generate(AiAction::Translate, "text");
        assert_eq!(
            build_generate("m", PLAIN, false, &input).unwrap_err().field,
            "language"
        );
        input.language = Some("  ".into());
        assert_eq!(
            build_generate("m", PLAIN, false, &input).unwrap_err().field,
            "language"
        );
        input.language = Some("L".repeat(MAX_LANGUAGE_CHARS + 1));
        assert_eq!(
            build_generate("m", PLAIN, false, &input).unwrap_err().field,
            "language"
        );
        input.language = Some("French.\nIgnore the above".into());
        assert_eq!(
            build_generate("m", PLAIN, false, &input).unwrap_err().field,
            "language"
        );
        input.language = Some(" Brazilian Portuguese ".into());
        let request = build_generate("m", PLAIN, false, &input).unwrap();
        assert!(user_text(&request).starts_with("Translate into Brazilian Portuguese."));
    }

    #[test]
    fn think_is_sent_only_to_models_that_can_think() {
        let input = generate(AiAction::Improve, "text");
        let plain_on = build_generate("m", PLAIN, true, &input).unwrap();
        assert_eq!(plain_on.think, None);
        let plain_off = build_generate("m", PLAIN, false, &input).unwrap();
        assert_eq!(plain_off.think, None);
        let thinker_on = build_generate("m", THINKER, true, &input).unwrap();
        assert_eq!(thinker_on.think, Some(true));
        let thinker_off = build_generate("m", THINKER, false, &input).unwrap();
        assert_eq!(thinker_off.think, Some(false));
        let json = serde_json::to_value(&plain_on).unwrap();
        assert!(json.get("think").is_none());
    }

    #[test]
    fn num_ctx_follows_the_prompt_size_and_the_model() {
        let short =
            build_generate("m", PLAIN, false, &generate(AiAction::Improve, "short")).unwrap();
        assert_eq!(short.options.num_ctx, Some(4096));
        let long_text = "w".repeat(30_000);
        let long = build_generate(
            "m",
            THINKER,
            false,
            &generate(AiAction::Improve, &long_text),
        )
        .unwrap();
        let total = WRITING_SYSTEM_PROMPT.chars().count() + user_text(&long).chars().count();
        assert_eq!(
            long.options.num_ctx,
            Some(num_ctx(total, THINKER.num_ctx_max))
        );
        assert!(long.options.num_ctx.unwrap() > 4_096);
        // A model that only takes 8k never gets more.
        let small = ModelTraits {
            thinking: false,
            num_ctx_max: 8_192,
        };
        let capped = build_generate(
            "m",
            small,
            false,
            &generate(AiAction::Improve, &"w".repeat(32_000)),
        )
        .unwrap();
        assert_eq!(capped.options.num_ctx, Some(8_192));
    }

    // ----- num_ctx -----

    #[test]
    fn num_ctx_is_clamped_and_rounded_up_to_a_multiple_of_1024() {
        // (prompt chars, model max, expected)
        let table = [
            (0, 32_768, 4_096),
            (3_000, 32_768, 4_096),
            (9_000, 32_768, 4_096),    // 3,000 + 1,024 = 4,024 -> 4,096
            (9_300, 32_768, 5_120),    // 3,100 + 1,024 = 4,124 -> 5,120
            (30_000, 32_768, 11_264),  // 11,024 -> 11,264
            (45_000, 32_768, 16_384),  // 16,024 -> 16,384
            (100_000, 32_768, 16_384), // capped at 16,384
            (100_000, 8_192, 8_192),   // capped by the model
            (100_000, 4_096, 4_096),
            (10, 2_048, 2_048), // a tiny model wins over the floor
            (10, 1, 1),
            (100_000, 0, 16_384), // unknown length
            (0, 0, 4_096),
            (usize::MAX, u32::MAX, 16_384),
        ];
        for (chars, max, expected) in table {
            assert_eq!(num_ctx(chars, max), expected, "num_ctx({chars}, {max})");
        }
    }

    #[test]
    fn num_ctx_is_always_a_multiple_of_1024_below_the_cap() {
        for chars in (0..60_000).step_by(997) {
            let value = num_ctx(chars, 32_768);
            assert!((4_096..=16_384).contains(&value));
            assert!(value.is_multiple_of(1_024));
        }
    }

    // ----- build_chat -----

    fn message(role: AiRole, content: &str) -> AiChatMessage {
        AiChatMessage {
            role,
            content: content.to_owned(),
        }
    }

    fn chat_input(messages: Vec<AiChatMessage>) -> AiChatInput {
        AiChatInput {
            messages,
            scope: AiScope::All,
        }
    }

    fn source(n: u32, title: &str, text: &str) -> AiSource {
        AiSource {
            n,
            page_id: PageId::from_bytes([n as u8; 16]),
            space_id: SpaceId::from_bytes([9; 16]),
            block_id: None,
            title: title.to_owned(),
            icon: None,
            text: text.to_owned(),
            score: 1.0,
        }
    }

    #[test]
    fn the_chat_system_prompt_lists_the_numbered_sources() {
        let input = chat_input(vec![message(AiRole::User, "When is the meeting?")]);
        let sources = [
            source(1, "Team notes", "The meeting moved to Thursday."),
            source(2, "", "An untitled passage."),
        ];
        let request = build_chat("qwen3:8b", PLAIN, false, &input, &sources).unwrap();
        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[0].role, "system");
        assert_eq!(
            request.messages[0].content,
            format!(
                "{CHAT_SYSTEM_PROMPT}\n\nSources:\n\n[1] Team notes\nThe meeting moved to Thursday.\n\n[2] Untitled\nAn untitled passage."
            )
        );
        assert_eq!(request.messages[1].role, "user");
        assert_eq!(request.messages[1].content, "When is the meeting?");
        assert!(request.stream);
        assert_eq!(request.options.temperature, Some(0.3));
    }

    #[test]
    fn the_chat_system_prompt_is_the_designed_one() {
        assert_eq!(
            CHAT_SYSTEM_PROMPT,
            "You answer questions about the user's notes. Use only the numbered sources below. Cite them inline as [1], [2]. If the sources do not contain the answer, say so plainly. Answer in the language of the question."
        );
    }

    #[test]
    fn a_chat_without_sources_says_so() {
        let input = chat_input(vec![message(AiRole::User, "Anything?")]);
        let request = build_chat("m", PLAIN, false, &input, &[]).unwrap();
        assert!(
            request.messages[0]
                .content
                .ends_with("Sources:\n(none found)")
        );
    }

    #[test]
    fn sources_are_numbered_by_their_own_n() {
        let input = chat_input(vec![message(AiRole::User, "q")]);
        let request = build_chat(
            "m",
            PLAIN,
            false,
            &input,
            &[source(5, "A", "a"), source(7, "B", "b")],
        )
        .unwrap();
        let system = &request.messages[0].content;
        assert!(system.contains("[5] A\na"));
        assert!(system.contains("[7] B\nb"));
    }

    #[test]
    fn the_conversation_keeps_roles_and_order() {
        let input = chat_input(vec![
            message(AiRole::User, "one"),
            message(AiRole::Assistant, "two"),
            message(AiRole::User, "three"),
        ]);
        let request = build_chat("m", PLAIN, false, &input, &[]).unwrap();
        let roles: Vec<&str> = request.messages.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["system", "user", "assistant", "user"]);
        let contents: Vec<&str> = request
            .messages
            .iter()
            .skip(1)
            .map(|m| m.content.as_str())
            .collect();
        assert_eq!(contents, vec!["one", "two", "three"]);
    }

    #[test]
    fn the_last_message_must_be_a_question_from_the_person() {
        let ends_with_assistant = chat_input(vec![
            message(AiRole::User, "q"),
            message(AiRole::Assistant, "a"),
        ]);
        assert_eq!(
            build_chat("m", PLAIN, false, &ends_with_assistant, &[])
                .unwrap_err()
                .field,
            "messages"
        );
        let empty = chat_input(vec![]);
        assert_eq!(
            build_chat("m", PLAIN, false, &empty, &[])
                .unwrap_err()
                .field,
            "messages"
        );
        let blank = chat_input(vec![message(AiRole::User, "  \n")]);
        assert_eq!(
            build_chat("m", PLAIN, false, &blank, &[])
                .unwrap_err()
                .field,
            "messages"
        );
    }

    #[test]
    fn message_limits_are_errors() {
        let many: Vec<_> = (0..=MAX_CHAT_MESSAGES)
            .map(|n| {
                message(
                    if n % 2 == 0 {
                        AiRole::User
                    } else {
                        AiRole::Assistant
                    },
                    "x",
                )
            })
            .collect();
        assert_eq!(many.len(), 41);
        assert_eq!(
            build_chat("m", PLAIN, false, &chat_input(many), &[])
                .unwrap_err()
                .field,
            "messages"
        );
        let exactly: Vec<_> = (0..MAX_CHAT_MESSAGES)
            .map(|n| {
                message(
                    if n % 2 == 0 {
                        AiRole::Assistant
                    } else {
                        AiRole::User
                    },
                    "x",
                )
            })
            .collect();
        assert!(build_chat("m", PLAIN, false, &chat_input(exactly), &[]).is_ok());
        let long = chat_input(vec![message(
            AiRole::User,
            &"x".repeat(MAX_MESSAGE_CHARS + 1),
        )]);
        assert_eq!(
            build_chat("m", PLAIN, false, &long, &[]).unwrap_err().field,
            "messages"
        );
        let at_limit = chat_input(vec![message(AiRole::User, &"é".repeat(MAX_MESSAGE_CHARS))]);
        assert!(build_chat("m", THINKER, false, &at_limit, &[]).is_ok());
        let old_long = chat_input(vec![
            message(AiRole::Assistant, &"x".repeat(MAX_MESSAGE_CHARS + 1)),
            message(AiRole::User, "q"),
        ]);
        assert_eq!(
            build_chat("m", PLAIN, false, &old_long, &[])
                .unwrap_err()
                .field,
            "messages"
        );
    }

    fn big_conversation() -> AiChatInput {
        // Six messages of 15,000 characters: 90,000 in all, over the 64,000 limit.
        let mut messages = Vec::new();
        for n in 0..6 {
            let role = if n % 2 == 0 {
                AiRole::User
            } else {
                AiRole::Assistant
            };
            messages.push(message(role, &format!("{n}{}", "x".repeat(14_999))));
        }
        messages.push(message(AiRole::User, "the question"));
        chat_input(messages)
    }

    #[test]
    fn the_oldest_messages_are_dropped_first_when_the_chat_is_too_long() {
        let request = build_chat("m", THINKER, false, &big_conversation(), &[]).unwrap();
        let kept: Vec<&str> = request
            .messages
            .iter()
            .skip(1)
            .map(|m| m.content.as_str())
            .collect();
        let kept_chars: usize = kept.iter().map(|c| c.chars().count()).sum();
        assert!(kept_chars <= MAX_CHAT_CHARS, "{kept_chars}");
        // Dropped from the front, in order, and the question is last.
        assert_eq!(kept.last().copied(), Some("the question"));
        assert!(kept.len() < 7);
        // What stays is the newest suffix, starting on a user message.
        assert_eq!(request.messages[1].role, "user");
        let first_kept = kept[0].chars().next().unwrap();
        assert!(first_kept.is_ascii_digit());
    }

    #[test]
    fn dropping_also_leaves_room_for_an_answer_in_the_models_context() {
        let small = ModelTraits {
            thinking: false,
            num_ctx_max: 4_096,
        }; // (4,096 - 1,024) * 3 = 9,216
        let request = build_chat("m", small, false, &big_conversation(), &[]).unwrap();
        let kept: Vec<&str> = request
            .messages
            .iter()
            .skip(1)
            .map(|m| m.content.as_str())
            .collect();
        assert_eq!(kept, vec!["the question"]);
        assert_eq!(request.options.num_ctx, Some(4_096));
    }

    #[test]
    fn nothing_is_dropped_when_it_fits() {
        let input = chat_input(vec![
            message(AiRole::Assistant, "hello"),
            message(AiRole::User, "q"),
        ]);
        let request = build_chat("m", PLAIN, false, &input, &[]).unwrap();
        assert_eq!(request.messages.len(), 3);
        assert_eq!(request.messages[1].role, "assistant");
    }

    #[test]
    fn the_question_is_kept_even_when_it_alone_is_over_the_budget() {
        let small = ModelTraits {
            thinking: false,
            num_ctx_max: 2_048,
        };
        let input = chat_input(vec![message(AiRole::User, &"q".repeat(15_000))]);
        let request = build_chat("m", small, false, &input, &[]).unwrap();
        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.options.num_ctx, Some(2_048));
    }

    #[test]
    fn chat_think_and_num_ctx_follow_the_traits() {
        let input = chat_input(vec![message(AiRole::User, "q")]);
        assert_eq!(
            build_chat("m", PLAIN, true, &input, &[]).unwrap().think,
            None
        );
        assert_eq!(
            build_chat("m", THINKER, true, &input, &[]).unwrap().think,
            Some(true)
        );
        assert_eq!(
            build_chat("m", THINKER, false, &input, &[]).unwrap().think,
            Some(false)
        );
        let sources: Vec<AiSource> = (1..=8)
            .map(|n| source(n, "Page", &"t".repeat(1_500)))
            .collect();
        let request = build_chat("m", THINKER, false, &input, &sources).unwrap();
        let total: usize = request
            .messages
            .iter()
            .map(|m| m.content.chars().count())
            .sum();
        assert_eq!(
            request.options.num_ctx,
            Some(num_ctx(total, THINKER.num_ctx_max))
        );
        assert!(request.options.num_ctx.unwrap() >= 5_120);
    }

    // ----- transcription -----

    #[test]
    fn a_transcription_request_carries_the_wav_as_base64() {
        let wav = [0u8, 1, 2, 250, 251, 252, 253];
        let request = build_transcription("r2t2", &wav);
        assert_eq!(request.model, "r2t2");
        assert!(!request.stream);
        assert_eq!(request.think, None);
        assert_eq!(request.options.temperature, Some(0.0));
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].role, "user");
        assert_eq!(
            request.messages[0].content,
            "Transcribe the speech in this audio clip verbatim. Output only the transcript."
        );
        assert_eq!(request.messages[0].images, vec!["AAEC+vv8/Q=="]);
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["stream"], serde_json::json!(false));
        assert_eq!(json["options"]["temperature"], serde_json::json!(0.0));
        assert!(json.get("think").is_none());
    }

    #[test]
    fn the_transcription_base64_is_standard_with_padding() {
        let request = build_transcription("m", &[0xff, 0xfe]);
        assert_eq!(request.messages[0].images[0], "//4=");
        let empty = build_transcription("m", &[]);
        assert_eq!(empty.messages[0].images[0], "");
    }

    // ----- summary_parts -----

    #[test]
    fn a_short_transcript_is_one_part_unchanged() {
        assert_eq!(summary_parts("hello\n\nworld"), vec!["hello\n\nworld"]);
        assert_eq!(summary_parts(""), vec![""]);
        let at_limit = "a".repeat(SUMMARY_PART_CHARS);
        assert_eq!(summary_parts(&at_limit), vec![at_limit]);
    }

    #[test]
    fn a_long_transcript_is_cut_at_paragraph_boundaries() {
        let paragraph = |c: char| c.to_string().repeat(9_000);
        let transcript = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|s| paragraph(s.chars().next().unwrap()))
            .collect::<Vec<_>>()
            .join("\n\n");
        let parts = summary_parts(&transcript);
        // Two paragraphs fit in a part (18,002), three do not (27,004).
        assert_eq!(parts.len(), 3);
        assert_eq!(
            parts[0],
            format!("{}\n\n{}", paragraph('a'), paragraph('b'))
        );
        assert_eq!(
            parts[1],
            format!("{}\n\n{}", paragraph('c'), paragraph('d'))
        );
        assert_eq!(parts[2], paragraph('e'));
        for part in &parts {
            assert!(part.chars().count() <= SUMMARY_PART_CHARS);
        }
        // Nothing lost.
        assert_eq!(parts.join("\n\n"), transcript);
    }

    #[test]
    fn a_paragraph_over_the_limit_is_cut_at_a_line_break_then_a_space_then_the_limit() {
        // Lines of 10,000 characters inside one paragraph (no blank line).
        let line = "l".repeat(10_000);
        let by_lines = format!("{line}\n{line}\n{line}\n{line}");
        let parts = summary_parts(&by_lines);
        assert!(parts.len() >= 2);
        assert!(
            parts
                .iter()
                .all(|p| p.chars().count() <= SUMMARY_PART_CHARS)
        );
        assert_eq!(
            parts.iter().map(|p| p.matches('l').count()).sum::<usize>(),
            40_000
        );

        let words = "word ".repeat(10_000); // 50,000 characters, spaces only
        let parts = summary_parts(&words);
        assert!(parts.len() >= 3);
        assert!(
            parts
                .iter()
                .all(|p| p.chars().count() <= SUMMARY_PART_CHARS)
        );
        assert!(parts.iter().all(|p| p.split(' ').all(|w| w == "word")));

        let solid = "z".repeat(60_000);
        let parts = summary_parts(&solid);
        assert_eq!(
            parts.iter().map(|p| p.chars().count()).collect::<Vec<_>>(),
            vec![24_000, 24_000, 12_000]
        );
    }

    #[test]
    fn parts_never_cut_inside_a_multi_byte_character() {
        let solid = "日".repeat(60_000);
        let parts = summary_parts(&solid);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].chars().count(), SUMMARY_PART_CHARS);
        assert_eq!(parts.concat(), solid);
    }

    #[test]
    fn a_transcript_markdown_of_a_long_meeting_splits_between_segments() {
        let segment = "s".repeat(5_000);
        let segments: Vec<(u64, String)> =
            (0..10u64).map(|n| (n * 30_000, segment.clone())).collect();
        let transcript = crate::transcript::transcript_markdown(&segments);
        let parts = summary_parts(&transcript);
        assert!(parts.len() >= 2);
        for part in &parts {
            assert!(part.starts_with("**["), "{}", &part[..20]);
            assert!(part.chars().count() <= SUMMARY_PART_CHARS);
        }
    }

    // ----- retrieval_query -----

    #[test]
    fn the_query_is_the_last_user_message() {
        let input = chat_input(vec![message(AiRole::User, "  What is the plan? ")]);
        assert_eq!(retrieval_query(&input), "What is the plan?");
    }

    #[test]
    fn a_follow_up_is_prefixed_by_the_previous_question() {
        let input = chat_input(vec![
            message(AiRole::User, "Who leads the Atlas project?"),
            message(AiRole::Assistant, "Dana leads it [1]."),
            message(AiRole::User, "And when does it ship?"),
        ]);
        assert_eq!(
            retrieval_query(&input),
            "Who leads the Atlas project?\nAnd when does it ship?"
        );
    }

    #[test]
    fn only_the_previous_user_message_is_used() {
        let input = chat_input(vec![
            message(AiRole::User, "first question"),
            message(AiRole::Assistant, "answer"),
            message(AiRole::User, "second question"),
            message(AiRole::Assistant, "answer"),
            message(AiRole::User, "third question"),
        ]);
        assert_eq!(retrieval_query(&input), "second question\nthird question");
    }

    #[test]
    fn the_query_fits_the_cores_limit_and_favours_the_last_message() {
        let long_previous = "p".repeat(5_000);
        let input = chat_input(vec![
            message(AiRole::User, &long_previous),
            message(AiRole::User, &"q".repeat(300)),
        ]);
        let query = retrieval_query(&input);
        assert_eq!(query.chars().count(), MAX_RETRIEVAL_QUERY_CHARS);
        assert!(query.ends_with(&"q".repeat(300)));
        assert!(query.starts_with("ppp"));

        let huge_last = chat_input(vec![
            message(AiRole::User, "earlier question that no longer matters"),
            message(AiRole::User, &"é".repeat(9_000)),
        ]);
        let query = retrieval_query(&huge_last);
        assert_eq!(query.chars().count(), MAX_RETRIEVAL_QUERY_CHARS);
        assert!(!query.contains('\n'));

        // Almost no room left: the previous question is not worth a fragment.
        let nearly = chat_input(vec![
            message(AiRole::User, "earlier question"),
            message(AiRole::User, &"w".repeat(1_990)),
        ]);
        assert_eq!(retrieval_query(&nearly), "w".repeat(1_990));
    }

    #[test]
    fn no_user_message_means_no_query() {
        assert_eq!(retrieval_query(&chat_input(vec![])), "");
        assert_eq!(
            retrieval_query(&chat_input(vec![message(AiRole::Assistant, "hi")])),
            ""
        );
        assert_eq!(
            retrieval_query(&chat_input(vec![message(AiRole::User, "  ")])),
            ""
        );
    }

    // ----- clean_output -----

    #[test]
    fn think_blocks_are_removed() {
        assert_eq!(
            clean_output("<think>let me see</think>The answer."),
            "The answer."
        );
        assert_eq!(
            clean_output("<think>\nmulti\nline\n</think>\n\nThe answer.\n"),
            "The answer."
        );
        assert_eq!(clean_output("Before <think>x</think>after"), "Before after");
        assert_eq!(
            clean_output("<think>a</think>One<think>b</think> two"),
            "One two"
        );
    }

    #[test]
    fn an_unterminated_think_block_swallows_the_rest() {
        assert_eq!(clean_output("<think>still thinking about it"), "");
        assert_eq!(
            clean_output("Partial answer.<think>and then"),
            "Partial answer."
        );
        assert_eq!(clean_output("  \n<think>x"), "");
    }

    #[test]
    fn reasoning_before_a_stray_closing_tag_goes() {
        assert_eq!(clean_output("text </think> more"), "more");
        assert_eq!(
            clean_output(
                "Okay, the user wants a shorter text.
</think>

Short text."
            ),
            "Short text."
        );
    }

    #[test]
    fn a_wrapping_fence_is_removed() {
        assert_eq!(
            clean_output("```markdown\n# Title\n\nBody\n```"),
            "# Title\n\nBody"
        );
        assert_eq!(clean_output("```\nBody\n```"), "Body");
        assert_eq!(clean_output("```md\nBody\n```\n"), "Body");
        assert_eq!(clean_output("```Markdown\nBody\n```"), "Body");
        assert_eq!(clean_output("  \n```markdown\r\nBody\r\n```  \n"), "Body");
        assert_eq!(
            clean_output("<think>x</think>```markdown\nBody\n```"),
            "Body"
        );
    }

    #[test]
    fn fences_that_are_part_of_the_answer_stay() {
        let code = "```rust\nfn main() {}\n```";
        assert_eq!(clean_output(code), code);
        let mixed = "Here is code:\n\n```\nlet x = 1;\n```";
        assert_eq!(clean_output(mixed), mixed);
        let two_blocks = "```\na\n```\n\ntext\n\n```\nb\n```";
        assert_eq!(clean_output(two_blocks), two_blocks);
        let inline = "```not a fence```";
        assert_eq!(clean_output(inline), inline);
        let unterminated = "```markdown\nBody without end";
        assert_eq!(clean_output(unterminated), unterminated);
    }

    #[test]
    fn a_markdown_wrapper_may_hold_balanced_code_blocks() {
        let answer = "```markdown\nIntro\n\n```rust\nfn f() {}\n```\n\nOutro\n```";
        assert_eq!(
            clean_output(answer),
            "Intro\n\n```rust\nfn f() {}\n```\n\nOutro"
        );
    }

    #[test]
    fn only_one_fence_is_removed() {
        let nested = "```markdown\n```\ninner\n```\n```";
        assert_eq!(clean_output(nested), "```\ninner\n```");
    }

    #[test]
    fn plain_text_is_only_trimmed() {
        assert_eq!(clean_output("  Hello.  \n"), "Hello.");
        assert_eq!(clean_output(""), "");
    }
}
