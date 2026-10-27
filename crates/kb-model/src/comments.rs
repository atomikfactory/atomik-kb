//! Comment records and the fold of a page's comment stream (M13 slice 2, ADR-0048,
//! `docs/security/sharing.md` section 8, `docs/development/milestone-13.md` sections 2.1 and 2.2).
//!
//! A comment is not a CRDT: it is a **signed record** in a stream of its own (the page's comment
//! stream, `DocKind::Comments`, engine [`kb_doc::ENGINE_RECORDS`]), so a Commenter writes comments
//! without being able to write the page, and only an author can change their own comment. This
//! module is pure: it knows the record, its canonical bytes and the rules that decide which records
//! count. It signs nothing and verifies nothing (`kb-model` has no key material): the core signs the
//! [`CommentRecord::signing_bytes`] under `kb_types::comments::COMMENT_RECORD_CONTEXT`, and before a
//! record reaches [`CommentStream::apply`] it has checked the signature against the author's device
//! key, that `author` is the account that device belongs to, that the record's page is the stream's
//! page, and the role the author held at the feed cursor of the item that carried the record
//! ([`VerifiedRecord::author_role`]).
//!
//! # The record
//!
//! `v1 | record_id | page_id | thread_id | comment_id | op | author | device | rev | at | sig`, all
//! integers little-endian, ids as their 16 bytes:
//!
//! | Part | Bytes |
//! |---|---|
//! | version | `1` byte, [`RECORD_VERSION`] |
//! | `record_id`, `page_id`, `thread_id`, `comment_id` | 16 each |
//! | op | one byte ([`CommentOp::code`]: 1 `Post`, 2 `Edit`, 3 `Delete`, 4 `Resolve`), then its payload |
//! | `Post` payload | anchor flag `0` or `1`, then (when `1`) `u32` length and the anchor as JSON, then `u32` length and the body as JSON |
//! | `Edit` payload | `u32` length and the body as JSON |
//! | `Delete` payload | none |
//! | `Resolve` payload | one byte, `0` reopen or `1` resolve |
//! | `author` (an account id), `device` | 16 each |
//! | `rev` | `u32`: 0 for a `Post`, and for an `Edit` the number of edits of the comment its author had seen, plus one |
//! | `at` | `i64`, Unix ms on the author's device (a claim, not proof) |
//! | `sig` | 64 bytes |
//!
//! `sig` signs every byte before it (that is [`CommentRecord::signing_bytes`]); the stream's update
//! blob is `kb_doc::encode_records_update(record.encode())`.
//!
//! # The rules (ADR-0048, decision 2)
//!
//! Which records count depends on the set of records only, never on the order they arrived in
//! (the property tests of `tests/comment_fold.rs` hold the fold to that). Every record needs an author
//! whose role at the record's cursor was Commenter or up (a Viewer's record of any op is ignored);
//! the rest of its rule is:
//!
//! | Op | Counts when |
//! |---|---|
//! | `Post` (a root when `thread_id == comment_id`) | its body is valid (not empty, at most 10 KB); a reply names a thread whose root `Post` counts; of several `Post`s for one `comment_id` the earliest `(at, record_id)` is the comment |
//! | `Edit` | its body is valid, its comment's `Post` counts and names the record's `thread_id`, the record's author is the comment's author, and no valid `Delete` of the comment exists; of several the last `(rev, at, device, record_id)` wins |
//! | `Delete` | its comment's `Post` counts and names the record's `thread_id`, and the record's author is the comment's author or held the Owner role; final |
//! | `Resolve` | the thread's root `Post` counts, `comment_id` is that root, and the record's author is the thread's author or held Editor or up; the last `(rev, at, device, record_id)` wins |
//!
//! A record that fails its rule is not an error: it is **ignored** by the fold (it may become valid
//! when an earlier record arrives, an `Edit` before its `Post`), and a snapshot never lists it. Every
//! record that counts is listed, the superseded `Edit`s and `Resolve`s too: dominance (record-set
//! inclusion, ADR-0045) compares record ids, so what a receiver saw valid must still be there.
//!
//! Decisions the ADR leaves open, fixed here:
//!
//! - **Delete is final.** An `Edit` of a deleted comment is ignored whenever it was written (the
//!   set has no cursors, so "later" cannot be told apart from "earlier"); the comment reads as a
//!   placeholder with no body while a later comment of its thread is live, and is dropped otherwise.
//!   A thread whose comments are all deleted leaves [`CommentStream::threads`]. Deleting the root does
//!   not freeze the thread: a `Resolve` is about the thread, not about the root's text.
//! - **One comment id, one `Post`.** Honest devices draw comment ids at random, so two `Post`s with
//!   one id are a forgery: the earliest `(at, record_id)` claim is the comment and the others are
//!   ignored. `at` is a claim, so a forger can backdate; the core therefore refuses (quarantines) a
//!   `Post` whose comment id the stream already defines ([`CommentStream::defines_comment`]) in feed
//!   order, which every device sees the same way. The fold only keeps the answer deterministic.
//! - **`at` is a claim, never proof.** The fold never lets a record predate what it refers to: a reply's
//!   `created_at` is at least its root's, and an `Edit`'s `edited_at` at least its comment's
//!   `created_at`. The records themselves are untouched.
//! - **Anchors.** The fold keeps a root's anchor as written when it is valid. One that cannot be
//!   resolved (a cursor that is not base64, a quote over 200 characters) does not cost the thread
//!   its place: the thread is kept as a page-level one (no block, no cursors) with its quote, cut to 200
//!   characters. Whether a valid anchor still points at something is the editor's business (it
//!   resolves cursors against the page). A reply carries no anchor, and one that has one is read
//!   without it.
//! - **Shape.** A record whose fields contradict each other (an `Edit` naming another thread than its
//!   comment's, a `Resolve` whose comment is not the thread's root) is ignored: honest clients never
//!   write one, and the derived tables index records by thread and comment.
//!
//! The record-set helpers [`snapshot_record_ids`], [`snapshot_dominates`] and
//! [`CommentStream::is_dominated_by`] are the inclusion check of snapshot dominance for a comment
//! stream.

use std::collections::{BTreeMap, BTreeSet};

use kb_types::comments::{COMMENT_BODY_MAX_BYTES, COMMENT_QUOTE_MAX_CHARS};
use kb_types::{
    AccountId, CommentAnchor, CommentBodyItem, CommentId, CommentRecordId, DeviceId, MemberRole,
    PageId, ThreadId,
};

/// The version byte of a record (`v1`).
pub const RECORD_VERSION: u8 = 1;
/// The length of a record's signature (Ed25519), in bytes.
pub const SIGNATURE_LEN: usize = 64;
/// The most an encoded anchor may weigh (JSON), in bytes: a block id, two cursors, a 200-character
/// quote.
pub const ANCHOR_MAX_BYTES: usize = 4 * 1024;

/// What a record does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentOp {
    /// Posts a comment: the root of a new thread when the record's `thread_id` is its `comment_id`
    /// (and then it may carry the anchor), a reply otherwise.
    Post {
        /// Where the thread points; only on a root.
        anchor: Option<CommentAnchor>,
        /// The body.
        body: Vec<CommentBodyItem>,
    },
    /// Replaces the body of the comment (the author only).
    Edit {
        /// The new body.
        body: Vec<CommentBodyItem>,
    },
    /// Deletes the comment (the author or an Owner); final.
    Delete,
    /// Resolves (`true`) or reopens (`false`) the thread (the thread's author, or Editor and up).
    Resolve {
        /// The new state.
        resolved: bool,
    },
}

impl CommentOp {
    /// The byte of the op in a record: 1 `Post`, 2 `Edit`, 3 `Delete`, 4 `Resolve`. On disk and on the
    /// wire: never renumber.
    #[must_use]
    pub const fn code(&self) -> u8 {
        match self {
            Self::Post { .. } => 1,
            Self::Edit { .. } => 2,
            Self::Delete => 3,
            Self::Resolve { .. } => 4,
        }
    }
}

/// One signed record of a comment stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentRecord {
    /// This record's id (random): what snapshot dominance counts.
    pub record_id: CommentRecordId,
    /// The page whose stream carries it; a record is valid in that page's stream only.
    pub page_id: PageId,
    /// The thread (`ThreadId::of(comment_id)` for a root).
    pub thread_id: ThreadId,
    /// The comment the record is about (for a `Resolve`, the thread's root comment).
    pub comment_id: CommentId,
    /// What it does.
    pub op: CommentOp,
    /// The author's account.
    pub author: AccountId,
    /// The device that signed it.
    pub device: DeviceId,
    /// Revision: 0 for a `Post`; an `Edit` or `Resolve` carries one more than the highest revision
    /// its device had seen for that comment (or thread).
    pub rev: u32,
    /// When the author's device made it, Unix ms.
    pub at: i64,
    /// The signature over [`Self::signing_bytes`]; all zero until the core signs it.
    pub sig: [u8; SIGNATURE_LEN],
}

/// A record cannot be read or fails a size or shape rule. The messages hold no user content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommentRecordError {
    /// The bytes are not a record: cut short, an unknown version or op, trailing bytes, a length
    /// beyond its limit, a body or anchor that is not JSON of the expected shape.
    #[error("malformed comment record: {0}")]
    Malformed(&'static str),
    /// The body is empty, over 10 KB serialised, or holds a text item that is empty.
    #[error("invalid comment body: {0}")]
    Body(&'static str),
    /// The anchor's quote is over 200 characters, or a cursor is not base64.
    #[error("invalid comment anchor: {0}")]
    Anchor(&'static str),
}

impl CommentRecord {
    /// Whether this record starts its thread (`thread_id` is the comment's own id).
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.thread_id == ThreadId::of(self.comment_id)
    }

    /// Every byte before the signature: what the core signs and verifies under
    /// `kb_types::comments::COMMENT_RECORD_CONTEXT`.
    #[must_use]
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(160);
        out.push(RECORD_VERSION);
        out.extend_from_slice(self.record_id.as_bytes());
        out.extend_from_slice(self.page_id.as_bytes());
        out.extend_from_slice(self.thread_id.as_bytes());
        out.extend_from_slice(self.comment_id.as_bytes());
        out.push(self.op.code());
        match &self.op {
            CommentOp::Post { anchor, body } => {
                match anchor {
                    Some(anchor) => {
                        out.push(1);
                        push_json(&mut out, anchor);
                    }
                    None => out.push(0),
                }
                push_json(&mut out, body);
            }
            CommentOp::Edit { body } => push_json(&mut out, body),
            CommentOp::Delete => {}
            CommentOp::Resolve { resolved } => out.push(u8::from(*resolved)),
        }
        out.extend_from_slice(self.author.as_bytes());
        out.extend_from_slice(self.device.as_bytes());
        out.extend_from_slice(&self.rev.to_le_bytes());
        out.extend_from_slice(&self.at.to_le_bytes());
        out
    }

    /// The whole record: [`Self::signing_bytes`] and the signature. The payload of the stream's
    /// update blob (`kb_doc::encode_records_update`).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.signing_bytes();
        out.extend_from_slice(&self.sig);
        out
    }

    /// Reads a record. Only the layout is checked (lengths, version, op, JSON shape, no trailing
    /// bytes); the signature, the author and the rules of the fold are the core's and the fold's.
    pub fn decode(bytes: &[u8]) -> Result<Self, CommentRecordError> {
        let mut reader = Reader { rest: bytes };
        if reader.byte()? != RECORD_VERSION {
            return Err(CommentRecordError::Malformed("unknown record version"));
        }
        let record_id = CommentRecordId::from_bytes(reader.array()?);
        let page_id = PageId::from_bytes(reader.array()?);
        let thread_id = ThreadId::from_bytes(reader.array()?);
        let comment_id = CommentId::from_bytes(reader.array()?);
        let op = match reader.byte()? {
            1 => {
                let anchor = match reader.byte()? {
                    0 => None,
                    1 => Some(reader.json::<CommentAnchor>(ANCHOR_MAX_BYTES)?),
                    _ => return Err(CommentRecordError::Malformed("unknown anchor flag")),
                };
                let body = reader.json::<Vec<CommentBodyItem>>(COMMENT_BODY_MAX_BYTES)?;
                CommentOp::Post { anchor, body }
            }
            2 => CommentOp::Edit {
                body: reader.json::<Vec<CommentBodyItem>>(COMMENT_BODY_MAX_BYTES)?,
            },
            3 => CommentOp::Delete,
            4 => CommentOp::Resolve {
                resolved: match reader.byte()? {
                    0 => false,
                    1 => true,
                    _ => return Err(CommentRecordError::Malformed("unknown resolve flag")),
                },
            },
            _ => return Err(CommentRecordError::Malformed("unknown op")),
        };
        let author = AccountId::from_bytes(reader.array()?);
        let device = DeviceId::from_bytes(reader.array()?);
        let rev = u32::from_le_bytes(reader.array()?);
        let at = i64::from_le_bytes(reader.array()?);
        let sig = reader.array::<SIGNATURE_LEN>()?;
        if !reader.rest.is_empty() {
            return Err(CommentRecordError::Malformed("trailing bytes"));
        }
        Ok(Self {
            record_id,
            page_id,
            thread_id,
            comment_id,
            op,
            author,
            device,
            rev,
            at,
            sig,
        })
    }
}

/// Appends `u32` length and the JSON of `value` (a value that cannot be serialised, which these
/// types never are, writes an empty one that [`CommentRecord::decode`] then refuses).
fn push_json<T: serde::Serialize>(out: &mut Vec<u8>, value: &T) {
    let json = serde_json::to_vec(value).unwrap_or_default();
    out.extend_from_slice(&u32::try_from(json.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&json);
}

/// A cursor over a record's bytes; every read is bounds-checked.
struct Reader<'a> {
    rest: &'a [u8],
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, CommentRecordError> {
        let [first] = self.array::<1>()?;
        Ok(first)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], CommentRecordError> {
        let (head, rest) = self
            .rest
            .split_first_chunk::<N>()
            .ok_or(CommentRecordError::Malformed("cut short"))?;
        self.rest = rest;
        Ok(*head)
    }

    /// `u32` length (at most `limit`) and that many bytes of JSON, read as a `T`.
    fn json<T: serde::de::DeserializeOwned>(
        &mut self,
        limit: usize,
    ) -> Result<T, CommentRecordError> {
        let len = usize::try_from(u32::from_le_bytes(self.array()?))
            .map_err(|_| CommentRecordError::Malformed("length"))?;
        if len > limit {
            return Err(CommentRecordError::Malformed("a field is over its limit"));
        }
        let (head, rest) = self
            .rest
            .split_at_checked(len)
            .ok_or(CommentRecordError::Malformed("cut short"))?;
        self.rest = rest;
        serde_json::from_slice(head)
            .map_err(|_| CommentRecordError::Malformed("not JSON of the expected shape"))
    }
}

/// Checks a body against the rules of `CommentPost` and `CommentEdit`: not empty, no text item that
/// is empty, at most `COMMENT_BODY_MAX_BYTES` (10 KB) serialised as JSON. Used by the core on the
/// way out (`InvalidInput` for `body`) and on every record that comes in.
pub fn validate_body(body: &[CommentBodyItem]) -> Result<(), CommentRecordError> {
    if body.is_empty() {
        return Err(CommentRecordError::Body("a comment has a body"));
    }
    if body
        .iter()
        .any(|item| matches!(item, CommentBodyItem::Text { text } if text.is_empty()))
    {
        return Err(CommentRecordError::Body("a text item is empty"));
    }
    let size = serde_json::to_vec(body).map_or(usize::MAX, |json| json.len());
    if size > COMMENT_BODY_MAX_BYTES {
        return Err(CommentRecordError::Body("the body is over 10 KB"));
    }
    Ok(())
}

/// Checks an anchor against the rules of `CommentPost`: the quote has at most
/// `COMMENT_QUOTE_MAX_CHARS` characters, and `start` and `end`, when present, are base64 text
/// (standard alphabet, `=` padding at the end only, a multiple of four characters, not empty). What
/// the cursors mean is the editor's business, never the core's.
pub fn validate_anchor(anchor: &CommentAnchor) -> Result<(), CommentRecordError> {
    if anchor.quote.chars().count() > COMMENT_QUOTE_MAX_CHARS {
        return Err(CommentRecordError::Anchor(
            "the quote is over 200 characters",
        ));
    }
    for cursor in [&anchor.start, &anchor.end].into_iter().flatten() {
        if !is_base64(cursor) {
            return Err(CommentRecordError::Anchor("a cursor is not base64"));
        }
    }
    Ok(())
}

/// Whether `text` is non-empty standard base64 with correct padding.
fn is_base64(text: &str) -> bool {
    let body = text.trim_end_matches('=');
    let padding = text.len() - body.len();
    !text.is_empty()
        && text.len().is_multiple_of(4)
        && padding <= 2
        && body
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/')
}

/// A record the core has checked, with what the fold cannot know: the role its author held at the
/// feed cursor of the item that carried it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRecord {
    /// The record.
    pub record: CommentRecord,
    /// The author's role in the space at the carrying item's cursor (`MemberRole::Viewer` for a
    /// record the timeline cannot place; the fold then ignores it, as it ignores every record of a
    /// Viewer).
    pub author_role: MemberRole,
}

/// A comment as the fold reads it (no names: the core adds them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldedComment {
    /// The comment.
    pub comment_id: CommentId,
    /// Its thread.
    pub thread_id: ThreadId,
    /// The author of the `Post`.
    pub author: AccountId,
    /// The body of the winning `Post` or `Edit`; empty for a deleted comment.
    pub body: Vec<CommentBodyItem>,
    /// The `at` of the `Post`, and for a reply at least its root's (a record never predates what
    /// it answers).
    pub created_at: i64,
    /// The `at` of the winning `Edit`, when there is one and the comment is not deleted, and at
    /// least `created_at`.
    pub edited_at: Option<i64>,
    /// A valid `Delete` exists.
    pub deleted: bool,
}

/// A thread as the fold reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldedThread {
    /// The thread (its root comment's id).
    pub thread_id: ThreadId,
    /// The page.
    pub page_id: PageId,
    /// The root's anchor; an anchor that cannot be resolved is a page-level one (no block, no
    /// cursors) that keeps the quote.
    pub anchor: Option<CommentAnchor>,
    /// The winning `Resolve` says resolved.
    pub resolved: bool,
    /// The author of the winning `Resolve` when resolved.
    pub resolved_by: Option<AccountId>,
    /// The comments: the root first, then by `(created_at, comment_id)`; a deleted comment is kept
    /// (as a placeholder) while a later comment of the thread is live, and dropped otherwise, so the
    /// list is empty only when every comment of the thread is deleted.
    pub comments: Vec<FoldedComment>,
}

/// The records of one page's comment stream and the views the fold computes from them.
///
/// The set holds every record the core verified, valid under the rules or not (a record that is
/// ignored now may count when an earlier one arrives). A record id is held once: the first record
/// with an id stays, a later one with the same id is not added ([`Self::apply`] answers `false`).
/// Records of another page are never added.
#[derive(Debug, Clone)]
pub struct CommentStream {
    page_id: PageId,
    records: BTreeMap<CommentRecordId, VerifiedRecord>,
}

impl CommentStream {
    /// An empty stream of `page_id`.
    #[must_use]
    pub fn new(page_id: PageId) -> Self {
        Self {
            page_id,
            records: BTreeMap::new(),
        }
    }

    /// A stream holding `records`, in any order (the result does not depend on it).
    #[must_use]
    pub fn from_records(
        page_id: PageId,
        records: impl IntoIterator<Item = VerifiedRecord>,
    ) -> Self {
        let mut stream = Self::new(page_id);
        for record in records {
            stream.apply(record);
        }
        stream
    }

    /// The page.
    #[must_use]
    pub const fn page_id(&self) -> PageId {
        self.page_id
    }

    /// Adds a record to the set. `false` when it is not added: the record is of another page, or its
    /// id is already there.
    pub fn apply(&mut self, record: VerifiedRecord) -> bool {
        if record.record.page_id != self.page_id
            || self.records.contains_key(&record.record.record_id)
        {
            return false;
        }
        self.records.insert(record.record.record_id, record);
        true
    }

    /// How many records the set holds, valid or not.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the set holds no record.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Whether the set holds this record id (valid or not).
    #[must_use]
    pub fn contains(&self, id: CommentRecordId) -> bool {
        self.records.contains_key(&id)
    }

    /// Every record of the set, by record id.
    pub fn records(&self) -> impl Iterator<Item = &VerifiedRecord> {
        self.records.values()
    }

    /// Whether a `Post` record of the set names this comment id, whether it counts or not. The core
    /// asks it, in feed order, before it applies a `Post`: a second `Post` for a comment id is a
    /// forgery (ids are random), and the core quarantines it instead of letting the fold pick between
    /// the two by `at`, which is only a claim.
    #[must_use]
    pub fn defines_comment(&self, id: CommentId) -> bool {
        self.records.values().any(|entry| {
            entry.record.comment_id == id && matches!(entry.record.op, CommentOp::Post { .. })
        })
    }

    /// The threads the rules leave, oldest root first (`created_at`, then thread id): what
    /// `CommentList` shows, without names. A thread whose comments are all deleted is left out.
    #[must_use]
    pub fn threads(&self) -> Vec<FoldedThread> {
        let fold = self.fold();
        let mut threads: Vec<(i64, FoldedThread)> = fold
            .posts
            .values()
            .filter(|post| post.record.is_root())
            .map(|root| (root.record.at, fold.thread(root)))
            .filter(|(_, thread)| !thread.comments.is_empty())
            .collect();
        threads.sort_by_key(|(created, thread)| (*created, thread.thread_id));
        threads.into_iter().map(|(_, thread)| thread).collect()
    }

    /// One thread (also a thread with no comment left standing, as `CommentDelete` answers it), or
    /// `None` when no valid root `Post` has this id.
    #[must_use]
    pub fn thread(&self, id: ThreadId) -> Option<FoldedThread> {
        let fold = self.fold();
        let root = fold.posts.get(&id.root_comment())?;
        root.record.is_root().then(|| fold.thread(root))
    }

    /// The author of a comment whose `Post` counts: who may edit it, and whom a `CommentDelete` or a
    /// reply notification names.
    #[must_use]
    pub fn comment_author(&self, id: CommentId) -> Option<AccountId> {
        self.fold().posts.get(&id).map(|post| post.record.author)
    }

    /// The ids of the records that count under the rules: what a snapshot lists, and what snapshot
    /// dominance (record-set inclusion, ADR-0045) compares.
    #[must_use]
    pub fn valid_record_ids(&self) -> BTreeSet<CommentRecordId> {
        self.fold().valid
    }

    /// The records that count under the rules, by record id: the content of a stream snapshot.
    #[must_use]
    pub fn valid_records(&self) -> Vec<&CommentRecord> {
        let valid = self.fold().valid;
        self.records
            .iter()
            .filter(|(id, _)| valid.contains(*id))
            .map(|(_, entry)| &entry.record)
            .collect()
    }

    /// Whether `snapshot` (the record ids of a stream snapshot, [`snapshot_record_ids`]) includes
    /// every record that counts here: the snapshot dominates what this device has seen
    /// ([`snapshot_dominates`]). Records the rules ignore are not owed to a snapshot.
    #[must_use]
    pub fn is_dominated_by(&self, snapshot: &BTreeSet<CommentRecordId>) -> bool {
        snapshot_dominates(snapshot, &self.valid_record_ids())
    }

    /// Applies the rules to the set (see the module documentation): the records that count and the
    /// state they leave. A pure function of the set.
    fn fold(&self) -> Fold<'_> {
        // 1. Which `Post`s can count on their own, and of those which one is each comment: the
        //    earliest claim by `(at, record_id)`, whatever the order the set was filled in.
        let mut claims: BTreeMap<CommentId, &VerifiedRecord> = BTreeMap::new();
        for entry in self.records.values() {
            if !post_stands_alone(entry) {
                continue;
            }
            claims
                .entry(entry.record.comment_id)
                .and_modify(|held| {
                    if post_order(&entry.record) < post_order(&held.record) {
                        *held = entry;
                    }
                })
                .or_insert(entry);
        }
        // 2. A reply counts when the root of its thread is a root that counts.
        let posts: BTreeMap<CommentId, &VerifiedRecord> = claims
            .iter()
            .filter(|(_, entry)| {
                entry.record.is_root()
                    || claims
                        .get(&entry.record.thread_id.root_comment())
                        .is_some_and(|root| root.record.is_root())
            })
            .map(|(id, entry)| (*id, *entry))
            .collect();
        let mut valid: BTreeSet<CommentRecordId> =
            posts.values().map(|entry| entry.record.record_id).collect();

        // 3. Deletes: the comment's author, or an Owner. Final.
        let mut deleted: BTreeSet<CommentId> = BTreeSet::new();
        for entry in self.records.values() {
            let record = &entry.record;
            if !matches!(record.op, CommentOp::Delete) || !entry.author_role.can_comment() {
                continue;
            }
            if let Some(post) = posts.get(&record.comment_id)
                && post.record.thread_id == record.thread_id
                && (record.author == post.record.author || entry.author_role.is_owner())
            {
                deleted.insert(record.comment_id);
                valid.insert(record.record_id);
            }
        }

        // 4. Edits (the comment's author, the comment not deleted) and resolves (the thread's author,
        //    or Editor and up): the last by `(rev, at, device, record_id)` wins.
        let mut edits: BTreeMap<CommentId, &CommentRecord> = BTreeMap::new();
        let mut resolves: BTreeMap<ThreadId, &CommentRecord> = BTreeMap::new();
        for entry in self.records.values() {
            let record = &entry.record;
            if !entry.author_role.can_comment() {
                continue;
            }
            match &record.op {
                CommentOp::Edit { body } => {
                    if validate_body(body).is_ok()
                        && !deleted.contains(&record.comment_id)
                        && posts.get(&record.comment_id).is_some_and(|post| {
                            post.record.thread_id == record.thread_id
                                && post.record.author == record.author
                        })
                    {
                        valid.insert(record.record_id);
                        keep_last(&mut edits, record.comment_id, record);
                    }
                }
                CommentOp::Resolve { .. } => {
                    if record.comment_id == record.thread_id.root_comment()
                        && posts.get(&record.comment_id).is_some_and(|root| {
                            root.record.is_root()
                                && (record.author == root.record.author
                                    || entry.author_role.can_edit())
                        })
                    {
                        valid.insert(record.record_id);
                        keep_last(&mut resolves, record.thread_id, record);
                    }
                }
                CommentOp::Post { .. } | CommentOp::Delete => {}
            }
        }

        // 5. The comments of each thread, for the views.
        let mut by_thread: BTreeMap<ThreadId, Vec<&VerifiedRecord>> = BTreeMap::new();
        for post in posts.values() {
            by_thread
                .entry(post.record.thread_id)
                .or_default()
                .push(post);
        }
        Fold {
            page_id: self.page_id,
            posts,
            by_thread,
            deleted,
            edits,
            resolves,
            valid,
        }
    }
}

/// What the rules leave of a set of records (see [`CommentStream::fold`]).
struct Fold<'a> {
    page_id: PageId,
    /// The `Post` that is each comment, for the comments that count.
    posts: BTreeMap<CommentId, &'a VerifiedRecord>,
    /// The `Post`s of each thread that counts.
    by_thread: BTreeMap<ThreadId, Vec<&'a VerifiedRecord>>,
    /// The comments with a valid `Delete`.
    deleted: BTreeSet<CommentId>,
    /// The winning valid `Edit` of each comment.
    edits: BTreeMap<CommentId, &'a CommentRecord>,
    /// The winning valid `Resolve` of each thread.
    resolves: BTreeMap<ThreadId, &'a CommentRecord>,
    /// The ids of every record that counts.
    valid: BTreeSet<CommentRecordId>,
}

impl Fold<'_> {
    /// The thread of a root `Post` that counts.
    fn thread(&self, root: &VerifiedRecord) -> FoldedThread {
        let thread_id = root.record.thread_id;
        // A reply never predates its root.
        let created_at = |post: &VerifiedRecord| post.record.at.max(root.record.at);
        let mut replies: Vec<&VerifiedRecord> = self
            .by_thread
            .get(&thread_id)
            .map(|posts| {
                posts
                    .iter()
                    .copied()
                    .filter(|post| post.record.comment_id != root.record.comment_id)
                    .collect()
            })
            .unwrap_or_default();
        replies.sort_by_key(|post| (created_at(post), post.record.comment_id));
        let mut comments: Vec<FoldedComment> = std::iter::once(root)
            .chain(replies)
            .map(|post| self.comment(post, created_at(post)))
            .collect();
        // A deleted comment stays as a placeholder while a later comment is live.
        let mut later_live = false;
        let mut kept: Vec<bool> = comments
            .iter()
            .rev()
            .map(|comment| {
                let kept = !comment.deleted || later_live;
                later_live |= !comment.deleted;
                kept
            })
            .collect();
        kept.reverse();
        let mut kept = kept.into_iter();
        comments.retain(|_| kept.next().unwrap_or(false));

        let (resolved, resolved_by) = match self.resolves.get(&thread_id) {
            Some(CommentRecord {
                op: CommentOp::Resolve { resolved: true },
                author,
                ..
            }) => (true, Some(*author)),
            _ => (false, None),
        };
        FoldedThread {
            thread_id,
            page_id: self.page_id,
            anchor: match &root.record.op {
                CommentOp::Post {
                    anchor: Some(anchor),
                    ..
                } => Some(resolvable(anchor)),
                _ => None,
            },
            resolved,
            resolved_by,
            comments,
        }
    }

    /// One comment: the `Post`'s body, or the winning `Edit`'s, and nothing for a deleted one.
    fn comment(&self, post: &VerifiedRecord, created_at: i64) -> FoldedComment {
        let record = &post.record;
        let deleted = self.deleted.contains(&record.comment_id);
        let edit = self.edits.get(&record.comment_id);
        let body = if deleted {
            Vec::new()
        } else {
            match (edit.map(|edit| &edit.op), &record.op) {
                (Some(CommentOp::Edit { body }), _) | (_, CommentOp::Post { body, .. }) => {
                    body.clone()
                }
                _ => Vec::new(),
            }
        };
        FoldedComment {
            comment_id: record.comment_id,
            thread_id: record.thread_id,
            author: record.author,
            body,
            created_at,
            edited_at: edit
                .filter(|_| !deleted)
                .map(|edit| edit.at.max(created_at)),
            deleted,
        }
    }
}

/// Whether a `Post` can count whatever else the set holds: its author could comment and its body is
/// valid. (A reply's anchor is read as if absent, an unresolvable root anchor degrades, see
/// [`resolvable`]: neither costs the comment.)
fn post_stands_alone(entry: &VerifiedRecord) -> bool {
    entry.author_role.can_comment()
        && matches!(&entry.record.op, CommentOp::Post { body, .. } if validate_body(body).is_ok())
}

/// The order that picks among `Post`s of one comment id: the earliest claim.
fn post_order(record: &CommentRecord) -> (i64, CommentRecordId) {
    (record.at, record.record_id)
}

/// The order of revisions of one comment or thread: the last wins.
fn revision_order(record: &CommentRecord) -> (u32, i64, DeviceId, CommentRecordId) {
    (record.rev, record.at, record.device, record.record_id)
}

/// Keeps in `map[key]` the record that is last by [`revision_order`].
fn keep_last<'a, K: Ord>(
    map: &mut BTreeMap<K, &'a CommentRecord>,
    key: K,
    record: &'a CommentRecord,
) {
    map.entry(key)
        .and_modify(|held| {
            if revision_order(record) > revision_order(held) {
                *held = record;
            }
        })
        .or_insert(record);
}

/// The anchor a thread shows: the one written when it is valid, otherwise a page-level anchor (no
/// block, no cursors) that keeps the quote, cut to its limit.
fn resolvable(anchor: &CommentAnchor) -> CommentAnchor {
    if validate_anchor(anchor).is_ok() {
        return anchor.clone();
    }
    CommentAnchor {
        block_id: None,
        start: None,
        end: None,
        quote: anchor.quote.chars().take(COMMENT_QUOTE_MAX_CHARS).collect(),
    }
}

/// The record ids of a stream snapshot's records (the `records` of `kb_doc::decode_records`), each
/// record read in full: a record that is not one refuses the whole snapshot. The ids are what
/// [`snapshot_dominates`] compares.
pub fn snapshot_record_ids<'a>(
    records: impl IntoIterator<Item = &'a [u8]>,
) -> Result<BTreeSet<CommentRecordId>, CommentRecordError> {
    records
        .into_iter()
        .map(|bytes| CommentRecord::decode(bytes).map(|record| record.record_id))
        .collect()
}

/// Snapshot dominance for a comment stream (ADR-0045, ADR-0048): a snapshot dominates what a device
/// has seen when its record set (`snapshot`) includes every record id the device saw count at feed
/// cursors up to what the snapshot covers (`seen`, [`CommentStream::valid_record_ids`] of the records
/// up to there). A snapshot that leaves one out is truncating and is refused.
#[must_use]
pub fn snapshot_dominates(
    snapshot: &BTreeSet<CommentRecordId>,
    seen: &BTreeSet<CommentRecordId>,
) -> bool {
    seen.is_subset(snapshot)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn record(op: CommentOp) -> CommentRecord {
        CommentRecord {
            record_id: CommentRecordId::from_bytes([1; 16]),
            page_id: PageId::from_bytes([2; 16]),
            thread_id: ThreadId::from_bytes([3; 16]),
            comment_id: CommentId::from_bytes([3; 16]),
            op,
            author: AccountId::from_bytes([4; 16]),
            device: DeviceId::from_bytes([5; 16]),
            rev: 7,
            at: 1_760_000_000_123,
            sig: [9; SIGNATURE_LEN],
        }
    }

    fn body(text: &str) -> Vec<CommentBodyItem> {
        vec![CommentBodyItem::Text { text: text.into() }]
    }

    #[test]
    fn every_op_round_trips_through_its_bytes() {
        let anchor = CommentAnchor {
            block_id: Some(kb_types::BlockId::from_bytes([6; 16])),
            start: Some("AQID".into()),
            end: None,
            quote: "plan".into(),
        };
        for op in [
            CommentOp::Post {
                anchor: Some(anchor),
                body: body("Looks good"),
            },
            CommentOp::Post {
                anchor: None,
                body: vec![
                    CommentBodyItem::Mention {
                        account_id: AccountId::from_bytes([8; 16]),
                    },
                    CommentBodyItem::Page {
                        id: PageId::from_bytes([9; 16]),
                    },
                ],
            },
            CommentOp::Edit {
                body: body("Better"),
            },
            CommentOp::Delete,
            CommentOp::Resolve { resolved: true },
            CommentOp::Resolve { resolved: false },
        ] {
            let record = record(op);
            let bytes = record.encode();
            assert_eq!(CommentRecord::decode(&bytes).unwrap(), record);
            // The signature covers every byte before it, and only those.
            assert_eq!(
                bytes[..bytes.len() - SIGNATURE_LEN],
                record.signing_bytes()[..]
            );
            assert_eq!(bytes[bytes.len() - SIGNATURE_LEN..], record.sig);
        }
    }

    #[test]
    fn a_record_is_a_root_when_its_thread_is_its_comment() {
        let mut root = record(CommentOp::Delete);
        assert!(root.is_root());
        root.thread_id = ThreadId::from_bytes([8; 16]);
        assert!(!root.is_root());
    }

    #[test]
    fn a_damaged_record_is_refused_never_a_panic() {
        let bytes = record(CommentOp::Edit { body: body("x") }).encode();
        for cut in 0..bytes.len() {
            assert!(
                CommentRecord::decode(&bytes[..cut]).is_err(),
                "cut at {cut}"
            );
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(CommentRecord::decode(&longer).is_err());
        let mut version = bytes.clone();
        version[0] = 2;
        assert!(CommentRecord::decode(&version).is_err());
        let mut op = bytes;
        op[1 + 16 * 4] = 9;
        assert!(CommentRecord::decode(&op).is_err());
    }

    #[test]
    fn bodies_and_anchors_obey_their_limits() {
        assert!(validate_body(&body("fine")).is_ok());
        assert!(validate_body(&[]).is_err());
        assert!(validate_body(&body("")).is_err());
        assert!(validate_body(&body(&"x".repeat(COMMENT_BODY_MAX_BYTES))).is_err());
        let anchor = |quote: String, start: Option<&str>| CommentAnchor {
            block_id: None,
            start: start.map(str::to_owned),
            end: None,
            quote,
        };
        assert!(validate_anchor(&anchor("q".repeat(200), Some("AQID"))).is_ok());
        assert!(validate_anchor(&anchor("é".repeat(200), None)).is_ok());
        assert!(validate_anchor(&anchor("q".repeat(201), None)).is_err());
        for bad in ["", "AQI", "A=QD", "AQ!D", "A===", "====", "AQID AQID"] {
            assert!(
                validate_anchor(&anchor(String::new(), Some(bad))).is_err(),
                "{bad:?}"
            );
        }
        assert!(validate_anchor(&anchor(String::new(), Some("AQI="))).is_ok());
        assert!(validate_anchor(&anchor(String::new(), Some("AQ=="))).is_ok());
    }

    #[test]
    fn the_stream_holds_each_record_id_once_and_only_its_own_page() {
        let page = PageId::from_bytes([2; 16]);
        let verified = |record: CommentRecord| VerifiedRecord {
            record,
            author_role: MemberRole::Commenter,
        };
        let mut stream = CommentStream::new(page);
        assert!(stream.is_empty());
        let first = record(CommentOp::Post {
            anchor: None,
            body: body("one"),
        });
        assert!(stream.apply(verified(first.clone())));
        assert!(!stream.apply(verified(first.clone())), "the same id twice");
        let mut other_page = first.clone();
        other_page.record_id = CommentRecordId::from_bytes([20; 16]);
        other_page.page_id = PageId::from_bytes([21; 16]);
        assert!(!stream.apply(verified(other_page)), "another page's record");
        assert_eq!(stream.len(), 1);
        assert!(stream.contains(first.record_id));
        let rebuilt = CommentStream::from_records(page, [verified(first)]);
        assert_eq!(rebuilt.len(), 1);
        assert_eq!(rebuilt.page_id(), page);
    }
}
