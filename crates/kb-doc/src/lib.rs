//! Engine boundary over Loro (ADR-0011, `docs/sync/crdt-strategy.md`).
//!
//! `kb-doc` owns everything about CRDT documents that is not specific to one document
//! type: the blob envelope every persisted or transmitted blob carries, import and
//! export, versions, and document creation with privacy-safe defaults (random peer
//! id per opened document, no timestamps). `kb-model` builds the typed documents
//! (account, structure, later page) on top of [`Doc`]; no crate above `kb-model`
//! sees a Loro type.
//!
//! Engine configuration (peer id, timestamps, fractional index of the page tree) is
//! per opened instance and never stored in the data, so [`Doc::new`] and
//! [`Doc::load`] apply the same configuration.

#![forbid(unsafe_code)]

mod history;
pub use history::{Frontiers, Run, Span};

use loro::{
    Counter, EncodedBlobMode, ExpandType, ExportMode, LoroDoc, PeerID, StyleConfig, StyleConfigMap,
    VersionVector,
};

/// Magic bytes opening every blob.
pub const BLOB_MAGIC: &[u8; 4] = b"KBD1";
/// Engine id of Loro in the envelope.
pub const ENGINE_LORO: u8 = 1;
/// Engine id of the records engine in the envelope (M13 slice 2, ADR-0048): the document is a set
/// of independent signed records, not a CRDT. An update blob holds one record and a snapshot blob the
/// set of valid records. [`decode_blob`] refuses it ([`DocError::UnsupportedEngine`]); read these with
/// [`decode_records`].
pub const ENGINE_RECORDS: u8 = 2;
/// Current encoding of our document shapes (bumped on breaking shape changes).
pub const FORMAT_VERSION: u16 = 1;
/// Oldest reader that can interpret blobs written by this version.
pub const MIN_READER_VERSION: u16 = 1;

/// Size of the envelope header: magic (4) + engine (1) + `format_version` (2, LE) +
/// `min_reader_version` (2, LE) + kind (1). The engine payload follows.
pub const HEADER_LEN: usize = 10;

/// Name of the page tree container of a [`DocKind::Structure`] document. Shared with
/// `kb-model` so the fractional-index configuration targets the right container.
pub const STRUCTURE_PAGES_TREE: &str = "pages";

/// Random bytes Loro appends to every fractional index it generates for the page tree.
///
/// Loro publishes no single recommended value; its movable-tree design notes give, for
/// a 99% chance that concurrent inserts at the same position get distinct keys, the
/// maximum number of concurrent inserts per jitter length: 1 byte -> 3, 2 bytes -> 37,
/// 3 bytes -> 582. Two bytes cover every realistic burst of a few devices (and the
/// small shared spaces planned later) for two extra bytes per create or move. A
/// collision is never a correctness problem (Loro orders equal keys by peer and
/// lamport); it only costs a re-spacing of the colliding siblings on a later insert.
pub const TREE_JITTER: u8 = 2;

/// How a text mark grows when text is typed at its edge (`docs/development/milestone-03.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkExpand {
    /// Text inserted right after the marked range inherits the mark.
    After,
    /// The mark never grows.
    None,
}

/// The marks of page document shape v1 with their expansion, in a fixed order. Part of
/// the persisted shape (ADR-0016): the editor configures its Loro replica identically
/// and a parity test compares both with `fixtures/doc/block-schema-v1.json`.
///
/// `textColor` and `highlight` (2026-10-07, additive: each carries one attribute,
/// `color`, a palette name) come after the six marks of the first table, so the canonical
/// order of the marks of older pages does not change.
pub const PAGE_MARKS: &[(&str, MarkExpand)] = &[
    ("bold", MarkExpand::After),
    ("italic", MarkExpand::After),
    ("underline", MarkExpand::After),
    ("strike", MarkExpand::After),
    ("code", MarkExpand::None),
    ("link", MarkExpand::None),
    ("textColor", MarkExpand::After),
    ("highlight", MarkExpand::After),
];

/// Envelope `kind` byte of a snapshot.
const KIND_SNAPSHOT: u8 = 0;
/// Envelope `kind` byte of an update.
const KIND_UPDATE: u8 = 1;

/// Errors from the engine boundary. Never a panic: corrupted or foreign data is
/// reported (threat model goal G8).
#[derive(Debug, thiserror::Error)]
pub enum DocError {
    /// Bytes are not one of our blobs (bad magic, truncated header, bad kind).
    #[error("corrupt document blob: {0}")]
    Corrupt(String),
    /// The blob was written by another engine.
    #[error("unsupported document engine {0}")]
    UnsupportedEngine(u8),
    /// The blob needs a newer reader than this build.
    #[error("document needs reader version {needed}, this build reads up to {supported}")]
    NeedsNewerReader {
        /// `min_reader_version` from the blob.
        needed: u16,
        /// [`FORMAT_VERSION`] of this build.
        supported: u16,
    },
    /// The engine rejected the payload (malformed update, unknown container).
    #[error("engine rejected the document data: {0}")]
    Engine(String),
    /// A version id that does not parse, or names operations the document does not have (page
    /// history, [`Frontiers`]).
    #[error("invalid version: {0}")]
    InvalidVersion(String),
}

/// What a blob holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobKind {
    /// A full document state; loading starts from the latest one.
    Snapshot,
    /// A delta since some version; replayed after the snapshot in order.
    Update,
}

impl BlobKind {
    const fn to_byte(self) -> u8 {
        match self {
            Self::Snapshot => KIND_SNAPSHOT,
            Self::Update => KIND_UPDATE,
        }
    }

    const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            KIND_SNAPSHOT => Some(Self::Snapshot),
            KIND_UPDATE => Some(Self::Update),
            _ => None,
        }
    }
}

/// The decoded envelope header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobHeader {
    /// Engine id ([`ENGINE_LORO`]).
    pub engine: u8,
    /// Shape encoding version.
    pub format_version: u16,
    /// Oldest reader that understands the payload.
    pub min_reader_version: u16,
    /// Snapshot or update.
    pub kind: BlobKind,
}

/// Which typed document a [`Doc`] backs. Fixes container layout and engine options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocKind {
    /// Per workspace: ordered list of spaces.
    Account,
    /// Per space: page tree, metadata, tombstones.
    Structure,
    /// Per page: block tree with rich text (Milestone 3).
    Page,
    /// Per page: the signed record stream of its comments (M13 slice 2, ADR-0048). Engine
    /// [`ENGINE_RECORDS`]: there is no Loro document behind it, so no [`Doc`] is ever made of this kind
    /// (its blobs go through [`encode_records_update`], [`encode_records_snapshot`] and
    /// [`decode_records`], and `kb-model` judges the records). The stream's document id derives from
    /// the page id (`kb_types::comments::COMMENT_STREAM_DOMAIN`).
    Comments,
}

/// An opaque encoded version vector. Compare with `==`; order is decided by the
/// engine, not by bytes.
///
/// [`Doc::version`] encodes canonically (entries sorted by peer), so two equal
/// version vectors produced by this crate have equal bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version(pub Vec<u8>);

impl Version {
    /// Whether this version vector includes every operation of `other` (`self >= other`): the test
    /// of snapshot dominance (ADR-0045): a snapshot whose version vector includes the end version
    /// vector ([`update_end_vv`]) of every entry it claims to cover is honest. A peer `other` does
    /// not mention counts as 0, so a vector with more peers than `other` still includes it; a peer
    /// `self` lacks or holds at a lower counter makes it `false`, and so does a vector that is
    /// concurrent with `other` (each knows something the other does not). Either side that does
    /// not decode is [`DocError::Corrupt`].
    pub fn includes(&self, other: &Version) -> Result<bool, DocError> {
        let ours = decode_version(self)?;
        let theirs = decode_version(other)?;
        Ok(theirs.iter().all(|(peer, end)| {
            // A counter of 0 (or below) is no operation at all.
            *end <= 0 || ours.get(peer).is_some_and(|have| have >= end)
        }))
    }
}

/// The version vector at the end of one engine **update** blob (an entry's update, as
/// [`Doc::export_updates_since`] writes it): the `end_vv` the core keeps per `(document, writer
/// device)` in `sync_entry_marks` for the snapshot dominance check (ADR-0045). It is a function of
/// the blob alone: the blob is parsed, not imported into a document. The vector names only the
/// peers whose operations the blob carries (a delta written after a merge may carry several), each
/// at the counter after its last operation in the blob, so it is compared with a snapshot's
/// [`Doc::version`] through [`Version::includes`], never with `==`. The bytes are canonical, like
/// [`Doc::version`]'s.
///
/// A snapshot blob, a blob that is not ours and one the engine rejects are [`DocError`]s
/// (`Corrupt`, `UnsupportedEngine`, `NeedsNewerReader`, `Engine`), and so is an update with no
/// operations (`Corrupt`).
pub fn update_end_vv(blob: &[u8]) -> Result<Version, DocError> {
    let (header, payload) = decode_blob(blob)?;
    if header.kind != BlobKind::Update {
        return Err(DocError::Corrupt(
            "a snapshot blob has no end version vector of its own".into(),
        ));
    }
    let meta = LoroDoc::decode_import_blob_meta(payload, true)
        .map_err(|e| DocError::Engine(e.to_string()))?;
    if meta.mode != EncodedBlobMode::Updates {
        // The envelope said update; the engine payload is a snapshot (or an outdated format).
        return Err(DocError::Corrupt(
            "the blob does not hold an engine update".into(),
        ));
    }
    let end = meta.partial_end_vv;
    if end.iter().all(|(_, counter)| *counter <= 0) {
        return Err(DocError::Corrupt("the update has no operations".into()));
    }
    Ok(Version(encode_version(&end)))
}

/// Wraps `BLOB_MAGIC | engine | format_version | min_reader_version | kind | payload`
/// (little-endian integers, fixed layout; see `docs/security/encryption-design.md`).
#[must_use]
pub fn encode_blob(kind: BlobKind, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(BLOB_MAGIC);
    out.push(ENGINE_LORO);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&MIN_READER_VERSION.to_le_bytes());
    out.push(kind.to_byte());
    out.extend_from_slice(payload);
    out
}

/// The most records one records snapshot may list (a corrupt count must not allocate).
pub const MAX_RECORDS_PER_BLOB: usize = 1 << 20;

/// A records blob taken apart: the envelope kind and the records it holds, in order (one for an
/// update, the set for a snapshot). The bytes are the records as `kb-model` encoded them; this crate
/// does not look inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordsBlob<'a> {
    /// Snapshot or update.
    pub kind: BlobKind,
    /// The records.
    pub records: Vec<&'a [u8]>,
}

/// An update blob of the records engine: the envelope with [`ENGINE_RECORDS`] and the kind
/// [`BlobKind::Update`], and exactly one non-empty record as its payload (an empty record is
/// refused when read back, so the writer is asked not to make one).
#[must_use]
pub fn encode_records_update(record: &[u8]) -> Vec<u8> {
    let mut out = records_header(BlobKind::Update, record.len());
    out.extend_from_slice(record);
    out
}

/// A snapshot blob of the records engine: the envelope with [`ENGINE_RECORDS`] and the kind
/// [`BlobKind::Snapshot`], then the number of records (`u32`, little-endian) and each record as its
/// length (`u32`, little-endian) and its bytes. The caller lists the records in the canonical order
/// it wants (equal sets in equal order give equal bytes). A record longer than `u32::MAX` bytes
/// cannot be written and is left out, which no real stream reaches (a stream is bounded by the
/// feed's quota, a record by the 10 KB body limit).
#[must_use]
pub fn encode_records_snapshot(records: &[&[u8]]) -> Vec<u8> {
    let fits: Vec<&[u8]> = records
        .iter()
        .copied()
        .filter(|record| u32::try_from(record.len()).is_ok())
        .collect();
    let size: usize = fits.iter().map(|record| 4 + record.len()).sum();
    let mut out = records_header(BlobKind::Snapshot, 4 + size);
    out.extend_from_slice(&u32::try_from(fits.len()).unwrap_or(u32::MAX).to_le_bytes());
    for record in fits {
        out.extend_from_slice(
            &u32::try_from(record.len())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        out.extend_from_slice(record);
    }
    out
}

/// The envelope of a records blob with room for `payload_len` more bytes.
fn records_header(kind: BlobKind, payload_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload_len);
    out.extend_from_slice(BLOB_MAGIC);
    out.push(ENGINE_RECORDS);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&MIN_READER_VERSION.to_le_bytes());
    out.push(kind.to_byte());
    out
}

/// Takes a records blob apart: the envelope is checked like [`decode_blob`] checks a Loro one (length,
/// magic, reader version, version sanity, kind), the engine must be [`ENGINE_RECORDS`]
/// ([`DocError::UnsupportedEngine`] otherwise, a Loro blob included), an update holds exactly one
/// non-empty record and a snapshot as many as its count says with nothing left over (anything else is
/// [`DocError::Corrupt`]). Never panics, whatever the input.
pub fn decode_records(bytes: &[u8]) -> Result<RecordsBlob<'_>, DocError> {
    let Some((header, payload)) = bytes.split_first_chunk::<HEADER_LEN>() else {
        return Err(DocError::Corrupt(format!(
            "blob of {} bytes is shorter than the {HEADER_LEN}-byte header",
            bytes.len()
        )));
    };
    let [m0, m1, m2, m3, engine, f0, f1, r0, r1, kind] = *header;
    if [m0, m1, m2, m3] != *BLOB_MAGIC {
        return Err(DocError::Corrupt("bad magic".into()));
    }
    if engine != ENGINE_RECORDS {
        return Err(DocError::UnsupportedEngine(engine));
    }
    let format_version = u16::from_le_bytes([f0, f1]);
    let min_reader_version = u16::from_le_bytes([r0, r1]);
    if min_reader_version > FORMAT_VERSION {
        return Err(DocError::NeedsNewerReader {
            needed: min_reader_version,
            supported: FORMAT_VERSION,
        });
    }
    if format_version == 0 || min_reader_version == 0 || min_reader_version > format_version {
        return Err(DocError::Corrupt(format!(
            "inconsistent versions (format {format_version}, min reader {min_reader_version})"
        )));
    }
    let Some(kind) = BlobKind::from_byte(kind) else {
        return Err(DocError::Corrupt(format!("unknown blob kind {kind}")));
    };
    match kind {
        BlobKind::Update => {
            if payload.is_empty() {
                return Err(DocError::Corrupt(
                    "an update holds one record, not none".into(),
                ));
            }
            Ok(RecordsBlob {
                kind,
                records: vec![payload],
            })
        }
        BlobKind::Snapshot => {
            let corrupt = || DocError::Corrupt("a records snapshot is cut short".into());
            let (count, mut rest) = payload.split_first_chunk::<4>().ok_or_else(corrupt)?;
            let count = usize::try_from(u32::from_le_bytes(*count)).map_err(|_| corrupt())?;
            if count > MAX_RECORDS_PER_BLOB {
                return Err(DocError::Corrupt(
                    "a records snapshot lists too many records".into(),
                ));
            }
            let mut records = Vec::with_capacity(count.min(1024));
            for _ in 0..count {
                let (len, tail) = rest.split_first_chunk::<4>().ok_or_else(corrupt)?;
                let len = usize::try_from(u32::from_le_bytes(*len)).map_err(|_| corrupt())?;
                if len == 0 || len > tail.len() {
                    return Err(corrupt());
                }
                let (record, tail) = tail.split_at(len);
                records.push(record);
                rest = tail;
            }
            if !rest.is_empty() {
                return Err(DocError::Corrupt(
                    "a records snapshot has trailing bytes".into(),
                ));
            }
            Ok(RecordsBlob { kind, records })
        }
    }
}

/// Validates the envelope and returns the header and the payload slice.
///
/// Checks, in order: length, magic, engine, reader version (a newer writer may use
/// kinds this build does not know, so [`DocError::NeedsNewerReader`] wins over an
/// unknown kind), version sanity, kind. Never panics, whatever the input.
pub fn decode_blob(bytes: &[u8]) -> Result<(BlobHeader, &[u8]), DocError> {
    let Some((header, payload)) = bytes.split_first_chunk::<HEADER_LEN>() else {
        return Err(DocError::Corrupt(format!(
            "blob of {} bytes is shorter than the {HEADER_LEN}-byte header",
            bytes.len()
        )));
    };
    let [m0, m1, m2, m3, engine, f0, f1, r0, r1, kind] = *header;
    if [m0, m1, m2, m3] != *BLOB_MAGIC {
        return Err(DocError::Corrupt("bad magic".into()));
    }
    if engine != ENGINE_LORO {
        return Err(DocError::UnsupportedEngine(engine));
    }
    let format_version = u16::from_le_bytes([f0, f1]);
    let min_reader_version = u16::from_le_bytes([r0, r1]);
    if min_reader_version > FORMAT_VERSION {
        return Err(DocError::NeedsNewerReader {
            needed: min_reader_version,
            supported: FORMAT_VERSION,
        });
    }
    if format_version == 0 || min_reader_version == 0 || min_reader_version > format_version {
        return Err(DocError::Corrupt(format!(
            "inconsistent versions (format {format_version}, min reader {min_reader_version})"
        )));
    }
    let Some(kind) = BlobKind::from_byte(kind) else {
        return Err(DocError::Corrupt(format!("unknown blob kind {kind}")));
    };
    Ok((
        BlobHeader {
            engine,
            format_version,
            min_reader_version,
            kind,
        },
        payload,
    ))
}

/// One CRDT document. Cheap to clone? No: one owner, mutations through `kb-model`.
pub struct Doc {
    kind: DocKind,
    doc: LoroDoc,
}

impl Doc {
    /// A new empty document. `peer` is a random `u64` from the injected `Rng`; it is
    /// never derived from device or account ids. Timestamps are disabled. Tree-based
    /// kinds enable Loro's fractional index with [`TREE_JITTER`] bytes of jitter for
    /// multi-peer editing.
    #[must_use]
    pub fn new(kind: DocKind, peer: u64) -> Self {
        let doc = Self::unconfigured(kind, peer);
        doc.configure();
        doc
    }

    /// Loads a document from an optional snapshot blob followed by update blobs in
    /// the order they were persisted. Every blob goes through [`decode_blob`].
    ///
    /// `peer` is the fresh random peer id of this opened instance (never the one that
    /// wrote the data). Updates whose dependencies are missing stay pending inside the
    /// engine (logged) instead of failing the load.
    pub fn load(
        kind: DocKind,
        peer: u64,
        snapshot: Option<&[u8]>,
        updates: &[Vec<u8>],
    ) -> Result<Self, DocError> {
        let doc = Self::unconfigured(kind, peer);
        if let Some(snapshot) = snapshot {
            doc.import_blob(snapshot)?;
        }
        for update in updates {
            doc.import_blob(update)?;
        }
        doc.configure();
        Ok(doc)
    }

    /// Imports one encoded blob (snapshot or update). Idempotent for already-known
    /// changes. Changes whose dependencies are missing wait inside the engine;
    /// [`import_checked`](Self::import_checked) tells.
    pub fn import(&self, blob: &[u8]) -> Result<(), DocError> {
        self.import_checked(blob).map(|_| ())
    }

    /// [`import`](Self::import) that answers whether the blob was applied in full:
    /// `false` when some of its changes wait inside the engine for dependencies this
    /// document does not have (history only a snapshot brings). They apply once it
    /// arrives; a snapshot exported meanwhile does not contain them.
    pub fn import_checked(&self, blob: &[u8]) -> Result<bool, DocError> {
        let was_empty = self.doc.oplog_vv().is_empty();
        let complete = self.import_blob(blob)?;
        if was_empty {
            // A snapshot imported into an empty document may replace the engine
            // state wholesale, configuration included.
            self.configure();
        }
        Ok(complete)
    }

    /// Everything after `since` (all history when `None`) as one encoded update
    /// blob, or `Ok(None)` when there is nothing new.
    ///
    /// Pending local operations are committed first, so they are always included.
    pub fn export_updates_since(
        &self,
        since: Option<&Version>,
    ) -> Result<Option<Vec<u8>>, DocError> {
        self.doc.commit();
        let from = match since {
            Some(version) => decode_version(version)?,
            None => VersionVector::default(),
        };
        if from.includes_vv(&self.doc.oplog_vv()) {
            return Ok(None);
        }
        let payload = self
            .doc
            .export(ExportMode::updates(&from))
            .map_err(|e| DocError::Engine(e.to_string()))?;
        Ok(Some(encode_blob(BlobKind::Update, &payload)))
    }

    /// The full current state as one encoded snapshot blob.
    pub fn export_snapshot(&self) -> Result<Vec<u8>, DocError> {
        self.doc.commit();
        let payload = self
            .doc
            .export(ExportMode::Snapshot)
            .map_err(|e| DocError::Engine(e.to_string()))?;
        Ok(encode_blob(BlobKind::Snapshot, &payload))
    }

    /// The current state as one encoded **snapshot blob without history** (a view link's publication,
    /// M13 slice 4, ADR-0051): the document as it reads now, plus nothing of what was deleted on the way
    /// (the engine's shallow snapshot at the current frontiers), so text deleted before the call is in
    /// neither the bytes nor a document imported from them. The result is an ordinary `Snapshot` blob
    /// (`KBD1` header, [`BlobKind::Snapshot`]) that [`Doc::load`] and [`Doc::import`] take into an empty
    /// document and the viewer's `loro-crdt` imports after the header's [`HEADER_LEN`] bytes. A document
    /// that imported it has no history before this point: it cannot merge a peer's concurrent edit of
    /// older content, which is why only a read-only viewer holds one. Pending local operations are
    /// committed first. Only [`DocKind::Page`] documents are published; any kind works here.
    ///
    /// The engine trims the history at the latest version that every operation precedes or follows. With
    /// one head that is the current version itself, so only its last operation (which carries no
    /// deleted text) is kept. With **several heads** (edits of two devices merged and not yet followed
    /// by a local edit) the trim would stop before the concurrent stretch and keep every operation of
    /// it, including text typed and deleted there. So the export then runs on a throwaway copy that
    /// first merges the heads with an operation of its own (a key written and removed in the root map
    /// `PUBLISH_MERGE_MAP`, which is then dropped from the exported state, so the published document reads
    /// like the original); the document itself is never written. The copy gets the engine's random peer id
    /// and records no timestamps. A reader that imports a publication must start from an empty document
    /// each time (a later publication is another history, not an update of the earlier one).
    pub fn export_shallow_snapshot(&self) -> Result<Vec<u8>, DocError> {
        self.doc.commit();
        if self.doc.oplog_frontiers().len() <= 1 {
            return shallow_payload(&self.doc);
        }
        let full = self
            .doc
            .export(ExportMode::Snapshot)
            .map_err(|e| DocError::Engine(e.to_string()))?;
        let merged = LoroDoc::new();
        merged.set_record_timestamp(false);
        merged
            .import(&full)
            .map_err(|e| DocError::Engine(e.to_string()))?;
        let map = merged.get_map(PUBLISH_MERGE_MAP);
        map.insert("merged", true)
            .map_err(|e| DocError::Engine(e.to_string()))?;
        merged.commit();
        map.delete("merged")
            .map_err(|e| DocError::Engine(e.to_string()))?;
        merged.commit();
        // The map is empty again; telling the engine so keeps it out of the exported state.
        merged.delete_root_container(loro::ContainerTrait::id(&map));
        shallow_payload(&merged)
    }

    /// Current version (encoded version vector), for `export_updates_since`.
    ///
    /// Commits pending local operations first, so the version never claims changes
    /// that a later export would not contain.
    #[must_use]
    pub fn version(&self) -> Version {
        self.doc.commit();
        Version(encode_version(&self.doc.oplog_vv()))
    }

    /// The document kind this was created with.
    #[must_use]
    pub fn kind(&self) -> DocKind {
        self.kind
    }

    /// The engine document. **For `kb-model` only**; nothing above it may call this.
    #[must_use]
    pub fn inner(&self) -> &loro::LoroDoc {
        &self.doc
    }

    /// The engine document with the privacy settings but without the per-kind
    /// configuration (applied after any import, see [`Self::configure`]).
    fn unconfigured(kind: DocKind, peer: u64) -> Self {
        let doc = LoroDoc::new();
        if let Err(err) = doc.set_peer_id(engine_peer(peer)) {
            // Unreachable after `engine_peer`; keep Loro's random peer rather than fail.
            tracing::warn!(error = %err, "could not set the document peer id");
        }
        doc.set_record_timestamp(false);
        Self { kind, doc }
    }

    /// Per-instance engine options that are not part of the data.
    ///
    /// For structure documents the page tree gets fractional indexes with
    /// [`TREE_JITTER`]. Loro's default tree configuration seeds the jitter generator
    /// with 0 on every replica, which would make all peers draw the same "random"
    /// bytes; disabling first makes `enable_fractional_index` re-seed it from this
    /// instance's peer id.
    ///
    /// Page documents get the mark expansion table [`PAGE_MARKS`]; marks outside it
    /// (written by a newer client) never expand. The expansion is recorded inside
    /// every mark operation, so this only governs operations created here.
    fn configure(&self) {
        match self.kind {
            DocKind::Structure => {
                let tree = self.doc.get_tree(STRUCTURE_PAGES_TREE);
                tree.disable_fractional_index();
                tree.enable_fractional_index(TREE_JITTER);
            }
            DocKind::Page => {
                let mut styles = StyleConfigMap::new();
                for (mark, expand) in PAGE_MARKS {
                    let expand = match expand {
                        MarkExpand::After => ExpandType::After,
                        MarkExpand::None => ExpandType::None,
                    };
                    styles.insert((*mark).into(), StyleConfig { expand });
                }
                self.doc.config_text_style(styles);
                self.doc.config_default_text_style(Some(StyleConfig {
                    expand: ExpandType::None,
                }));
            }
            // No engine document is configured for a records stream (see [`DocKind::Comments`]).
            DocKind::Account | DocKind::Comments => {}
        }
    }

    /// Imports one blob; `false` when some of its changes wait for missing
    /// dependencies.
    fn import_blob(&self, blob: &[u8]) -> Result<bool, DocError> {
        let (_header, payload) = decode_blob(blob)?;
        let status = self
            .doc
            .import(payload)
            .map_err(|e| DocError::Engine(e.to_string()))?;
        let complete = status.pending.is_none();
        if let Some(pending) = status.pending {
            tracing::debug!(?pending, "imported changes wait for missing dependencies");
        }
        Ok(complete)
    }
}

/// The root map of the throwaway copy [`Doc::export_shallow_snapshot`] merges concurrent heads in; it is
/// removed again before the export.
const PUBLISH_MERGE_MAP: &str = "kb-publish-merge";

/// The engine's shallow snapshot of `doc` at its current frontiers, in a `Snapshot` blob.
fn shallow_payload(doc: &LoroDoc) -> Result<Vec<u8>, DocError> {
    let frontiers = doc.oplog_frontiers();
    let payload = doc
        .export(ExportMode::shallow_snapshot(&frontiers))
        .map_err(|e| DocError::Engine(e.to_string()))?;
    Ok(encode_blob(BlobKind::Snapshot, &payload))
}

/// Loro reserves `PeerID::MAX` for internal use; a random peer hits it with
/// probability 2^-64, in which case the neighbouring value is used.
const fn engine_peer(peer: u64) -> PeerID {
    if peer == PeerID::MAX {
        PeerID::MAX - 1
    } else {
        peer
    }
}

/// Canonical encoding: entries sorted by peer, empty entries dropped, so equal
/// version vectors always produce equal bytes.
fn encode_version(vv: &VersionVector) -> Vec<u8> {
    let mut entries: Vec<(PeerID, Counter)> = vv
        .iter()
        .filter(|(_, counter)| **counter > 0)
        .map(|(peer, counter)| (*peer, *counter))
        .collect();
    entries.sort_unstable();
    entries.into_iter().collect::<VersionVector>().encode()
}

fn decode_version(version: &Version) -> Result<VersionVector, DocError> {
    VersionVector::decode(&version.0)
        .map_err(|_| DocError::Corrupt("invalid version vector".into()))
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_history;
