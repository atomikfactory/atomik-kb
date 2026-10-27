//! Shared limits of attachments (ADR-0020), used by the core, the shells and mirrored
//! by the TypeScript client. Not glob re-exported: refer to them as
//! `kb_types::attachments::...`.

/// Plaintext bytes per chunk; every chunk but the last is exactly this long. Must equal
/// `kb_crypto::CHUNK_SIZE` (the core asserts it).
pub const CHUNK_SIZE: usize = 4 * 1024 * 1024;
/// Largest attachment accepted (513 chunks: 512 full ones plus the final chunk).
pub const MAX_ATTACHMENT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Longest attachment name, in bytes of UTF-8.
pub const MAX_NAME_BYTES: usize = 255;
/// Longest MIME type, in bytes (printable ASCII with one `/`).
pub const MAX_MIME_BYTES: usize = 127;
/// Largest attachment the UI previews in the webview (the raw read answers at most this
/// without a byte range).
pub const MAX_PREVIEW_BYTES: u64 = 32 * 1024 * 1024;
/// Days an unreferenced attachment is kept before garbage collection, unless the setting
/// below says otherwise (`0` = keep forever).
pub const DEFAULT_RETENTION_DAYS: u32 = 30;
/// Setting key of the retention period (a UI setting the core reads at collection time).
pub const RETENTION_SETTING: &str = "app.attachments.retention_days";
/// Setting key of the download policy (ADR-0023): which chunks a device fetches.
pub const DOWNLOAD_POLICY_SETTING: &str = "app.attachments.download";
/// Download policy value: every chunk of every manifest that arrives is wanted at once
/// (the default on desktop).
pub const DOWNLOAD_POLICY_EAGER: &str = "eager";
/// Download policy value: chunks are wanted when the attachment is opened or
/// `AttachmentDownload` asks for it.
pub const DOWNLOAD_POLICY_ON_DEMAND: &str = "on_demand";
/// Domain prefix of the chunk AAD: `ATTACHMENT_AAD_DOMAIN || attachment id` (16 bytes).
pub const ATTACHMENT_AAD_DOMAIN: &[u8] = b"atomik-kb/v1/attachment/";
