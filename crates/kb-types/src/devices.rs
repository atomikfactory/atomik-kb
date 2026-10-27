//! Shared constants of device pairing, revocation and recovery (ADR-0022), used by
//! `kb-keys`, `kb-sync`, `kb-net`, `kb-server`, the core and the TypeScript client.
//! Not glob re-exported: refer to them as `kb_types::devices::...`.

/// Characters of a pairing code (Crockford base32: 48 bits of mailbox id and 12 bits
/// of checksum).
pub const PAIRING_CODE_LEN: usize = 12;
/// Bytes of a mailbox id.
pub const MAILBOX_ID_LEN: usize = 6;
/// How long a pairing mailbox lives on the server.
pub const PAIRING_TTL_SECONDS: u64 = 600;
/// Messages a mailbox holds at most.
pub const PAIRING_MAX_MESSAGES: u32 = 64;
/// Largest pairing message.
pub const PAIRING_MAX_MESSAGE_BYTES: usize = 16 * 1024;
/// Longest a message long-poll waits.
pub const PAIRING_POLL_SECONDS: u32 = 25;
/// Digits of the short authentication string.
pub const SAS_DIGITS: usize = 6;
/// Domain of the pairing transcript (prefix of what both sides hash).
pub const PAIRING_TRANSCRIPT_DOMAIN: &str = "atomik-kb/v1/pairing";
/// Domain of the pairing code checksum.
pub const PAIRING_CODE_DOMAIN: &str = "atomik-kb/v1/pairing-code";
/// Context of a root key box in a `RootRotated` log entry (the generation follows).
pub const ROOT_ROTATION_CONTEXT: &str = "atomik-kb/v1/root-rotation";
/// Largest recovery blob the server stores.
pub const RECOVERY_BLOB_MAX_BYTES: usize = 4 * 1024;
/// Device names are padded to this length inside pairing messages.
pub const DEVICE_NAME_PAD: usize = 64;
