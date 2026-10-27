//! Shared constants of the sync protocol (ADR-0021), used by `kb-sync`, `kb-net`,
//! `kb-server`, the core and the TypeScript client. Not glob re-exported: refer to them
//! as `kb_types::sync::...`.

/// Wire protocol version written in every header. Version 2 (ADR-0045) replaced version 1
/// without a server migration; a version 1 object is refused everywhere.
pub const PROTOCOL_VERSION: u8 = 2;
/// Largest entry, snapshot or chunk the server accepts.
pub const MAX_WIRE_BYTES: u64 = 16 * 1024 * 1024;
/// Entries per `changes` page.
pub const CHANGES_PAGE_LIMIT: u32 = 200;
/// A document gets a new remote snapshot after this many entries since the last one...
pub const SNAPSHOT_AFTER_ENTRIES: u64 = 500;
/// ...or after this many entry bytes.
pub const SNAPSHOT_AFTER_BYTES: u64 = 4 * 1024 * 1024;
/// Days the server keeps entries a snapshot covers before deleting them.
pub const PRUNE_GRACE_DAYS: u32 = 7;
/// Lifetime of a session token.
pub const SESSION_TTL_SECONDS: u64 = 86_400;
/// Lifetime of an authentication challenge.
pub const CHALLENGE_TTL_SECONDS: u64 = 120;
/// How long the driver waits after a local edit before pushing.
pub const PUSH_DEBOUNCE_MS: u64 = 500;
/// How often the driver pulls without a notification.
pub const PULL_INTERVAL_MS: u64 = 60_000;
/// The feed's document id of keyring entries (they belong to no document).
pub const KEYRING_DOC_ID: crate::DocId = crate::DocId::from_bytes([0u8; 16]);
/// Chunks a sync cycle transfers at most (ADR-0023); a cycle that used the whole
/// budget is followed by another at once.
pub const TRANSFER_BUDGET: usize = 8;
/// A chunk the server does not hold yet is asked for again after
/// `min(TRANSFER_RETRY_BASE_MS * 2^attempts, TRANSFER_RETRY_MAX_MS)`.
pub const TRANSFER_RETRY_BASE_MS: u64 = 60_000;
/// Upper bound of that delay.
pub const TRANSFER_RETRY_MAX_MS: u64 = 3_600_000;

/// What the driver does with one chunk (ADR-0023).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChunkWorkKind {
    /// The chunk is here and the server lacks it: `put_chunk`.
    Upload,
    /// The chunk is wanted here: `get_chunk`.
    Download,
    /// A purge here: `delete_chunk`.
    Delete,
}

/// One item of chunk work the core hands the driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkWork {
    /// The chunk.
    pub hash: crate::ChunkId,
    /// The attachment it belongs to.
    pub attachment_id: crate::AttachmentId,
    /// What to do.
    pub kind: ChunkWorkKind,
}

/// What a feed is: the `kind` column of `sync_feeds` (ADR-0045).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FeedKind {
    /// The account's own feed: its id is the account id.
    Personal,
    /// A shared space's feed: a random id.
    Shared,
}

impl FeedKind {
    /// The stored value: 1 personal, 2 shared. On disk: never renumber.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        match self {
            Self::Personal => 1,
            Self::Shared => 2,
        }
    }

    /// Parses [`Self::to_byte`]; any other value is `None`.
    #[must_use]
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Personal),
            2 => Some(Self::Shared),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_kinds_have_their_stored_values() {
        assert_eq!(FeedKind::Personal.to_byte(), 1);
        assert_eq!(FeedKind::Shared.to_byte(), 2);
        assert_eq!(FeedKind::from_byte(1), Some(FeedKind::Personal));
        assert_eq!(FeedKind::from_byte(2), Some(FeedKind::Shared));
        assert_eq!(FeedKind::from_byte(0), None);
        assert_eq!(FeedKind::from_byte(3), None);
        assert_eq!(PROTOCOL_VERSION, 2);
    }
}
