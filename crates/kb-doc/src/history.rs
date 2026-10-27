//! What page history needs of a document (ADR-0057, `docs/architecture/page-history.md`): the
//! **version id** (a document's frontiers written as text), a copy of the document as it was at a
//! version, making the document what it was at a version as one new commit, and the stretches of
//! history that no journal row accounts for (the undated versions). No Loro type crosses this
//! boundary: a version is a [`Frontiers`].

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use loro::{ID, PeerID};

use crate::{Doc, DocError, Version, decode_version, engine_peer};

/// A version of a document: the frontiers of its history, that is the operations nothing else in
/// the history follows. Every replica that has those operations has the same version.
///
/// The text form is the **version id**: each frontier `ID(peer, counter)` written
/// `"<peer>@<counter>"` (the peer in decimal), sorted by peer, joined with `,`
/// (`"8291736450123@57,991827364@1203"`). [`Frontiers::encode`] and [`Frontiers::parse`] convert;
/// [`Doc::resolve_frontiers`] also checks that the document has the operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frontiers(loro::Frontiers);

impl Frontiers {
    /// The version id: `"<peer>@<counter>"` sorted by peer and counter, joined with `,`. Empty
    /// for the version before everything (a document with no operations).
    #[must_use]
    pub fn encode(&self) -> String {
        let mut ids: Vec<(PeerID, i32)> = self.0.iter().map(|id| (id.peer, id.counter)).collect();
        ids.sort_unstable();
        let parts: Vec<String> = ids
            .iter()
            .map(|(peer, counter)| format!("{peer}@{counter}"))
            .collect();
        parts.join(",")
    }

    /// Reads a version id. Only the syntax is checked: digits, `@`, `,`, a peer at most once. The
    /// empty text is the version before everything. Whether a document has these operations is
    /// [`Doc::resolve_frontiers`]'s question.
    pub fn parse(text: &str) -> Result<Self, DocError> {
        if text.is_empty() {
            return Ok(Self(loro::Frontiers::default()));
        }
        let invalid = || DocError::InvalidVersion(format!("not a version id: {text:?}"));
        let mut peers: HashSet<PeerID> = HashSet::new();
        let mut ids: Vec<ID> = Vec::new();
        for part in text.split(',') {
            let (peer, counter) = part.split_once('@').ok_or_else(invalid)?;
            let peer = digits(peer).and_then(|d| d.parse::<PeerID>().ok());
            let counter = digits(counter).and_then(|d| d.parse::<i32>().ok());
            let (Some(peer), Some(counter)) = (peer, counter) else {
                return Err(invalid());
            };
            // `PeerID::MAX` is reserved by the engine.
            if peer == PeerID::MAX || !peers.insert(peer) {
                return Err(invalid());
            }
            ids.push(ID::new(peer, counter));
        }
        Ok(Self(ids.into_iter().collect()))
    }

    /// The version before everything (a document with no operations): the base of a session that
    /// began with the document.
    #[must_use]
    pub fn empty() -> Self {
        Self(loro::Frontiers::default())
    }

    /// This is the version before everything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The version of the single operation `peer@counter` and everything causally before it.
    fn of_operation(peer: PeerID, counter: i32) -> Self {
        Self(loro::Frontiers::from_id(ID::new(peer, counter)))
    }
}

impl std::fmt::Display for Frontiers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.encode())
    }
}

/// `text` when it is one or more ASCII digits and nothing else (`"+5"` and `"-1"` are not).
fn digits(text: &str) -> Option<&str> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())).then_some(text)
}

/// A stretch of history by one replica that no journal row accounts for: an **undated version**
/// (ADR-0057). A run is a sequence of operations of one peer that follow each other in the order
/// of the history, with no operation of another peer, and no operation a journal row covers, in
/// between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// The version right after the run's last operation: `[peer@last counter]` (everything before
    /// that operation included).
    pub version: Frontiers,
    /// The replica that made the run.
    pub peer: u64,
    /// The Lamport timestamp of the run's last operation (the order of history: higher is newer).
    pub lamport: u64,
    /// How many operations the run has.
    pub operations: usize,
}

/// The extent of one journal row for [`Doc::runs_outside`]: the operations of `end` that are not
/// in `base`, that is what its session wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// The version the session began at (the empty version for a document that began with it).
    pub base: Frontiers,
    /// The version the session ended at.
    pub end: Frontiers,
}

/// One stretch of one change of the history, covered by a journal row or not.
struct Piece {
    peer: PeerID,
    start: i32,
    end: i32,
    lamport_last: u64,
    covered: bool,
}

impl Doc {
    /// The document's version now (committing what is pending first): the frontiers of its
    /// history. Empty for a document with no operations.
    #[must_use]
    pub fn frontiers(&self) -> Frontiers {
        self.doc.commit();
        Frontiers(self.doc.oplog_frontiers())
    }

    /// The version a [`Doc::version`] vector stands for, as frontiers (the version this document
    /// had when `version` was taken). Operations of `version` that the history does not have are
    /// ignored.
    pub fn frontiers_at(&self, version: &Version) -> Result<Frontiers, DocError> {
        let vv = decode_version(version)?;
        self.doc.commit();
        Ok(Frontiers(self.doc.vv_to_frontiers(&vv)))
    }

    /// Reads a version id of this document: the syntax of [`Frontiers::parse`], and every
    /// operation it names must be in the history (else [`DocError::InvalidVersion`]). The empty
    /// id is refused (a restore to it would blank the document). The answer is canonical (the
    /// minimal frontiers of the same version), so equal versions have equal ids.
    pub fn resolve_frontiers(&self, text: &str) -> Result<Frontiers, DocError> {
        let parsed = Frontiers::parse(text)?;
        if parsed.is_empty() {
            return Err(DocError::InvalidVersion("the version id is empty".into()));
        }
        self.check_known(&parsed)?;
        let minimal = self.doc.minimize_frontiers(&parsed.0).map_err(|id| {
            DocError::InvalidVersion(format!("the document has no operation {id:?}"))
        })?;
        Ok(Frontiers(minimal))
    }

    /// `InvalidVersion` unless every operation of `at` is in the history (committing first).
    fn check_known(&self, at: &Frontiers) -> Result<(), DocError> {
        self.doc.commit();
        let vv = self.doc.oplog_vv();
        match at.0.iter().find(|id| !vv.includes_id(*id)) {
            Some(id) => Err(DocError::InvalidVersion(format!(
                "the document has no operation {id:?}"
            ))),
            None => Ok(()),
        }
    }

    /// How two versions of this document relate: `Some(Less)` when `a` is in `b`'s past,
    /// `Some(Greater)` when `b` is in `a`'s, `Some(Equal)` when they are the same, `None` when
    /// each has something the other lacks. `InvalidVersion` when an operation is not in the
    /// history.
    pub fn compare_frontiers(
        &self,
        a: &Frontiers,
        b: &Frontiers,
    ) -> Result<Option<Ordering>, DocError> {
        self.check_known(a)?;
        self.check_known(b)?;
        self.doc
            .cmp_frontiers(&a.0, &b.0)
            .map_err(|_| DocError::InvalidVersion("a version names unknown operations".into()))
    }

    /// A new document of the same kind with the history up to `at` and nothing after it: what the
    /// document read like at that version. The copy is read-only in spirit (nothing persists it);
    /// `peer` is its random replica id, as for [`Doc::new`]. `InvalidVersion` when `at` names
    /// operations the history does not have (resolve ids with [`Doc::resolve_frontiers`] first).
    pub fn fork_at(&self, at: &Frontiers, peer: u64) -> Result<Doc, DocError> {
        self.check_known(at)?;
        let forked = self
            .doc
            .fork_at(&at.0)
            .map_err(|e| DocError::Engine(e.to_string()))?;
        if let Err(err) = forked.set_peer_id(engine_peer(peer)) {
            // Unreachable after `engine_peer`; keep the engine's random peer rather than fail.
            tracing::warn!(error = %err, "could not set the peer id of a document copy");
        }
        forked.set_record_timestamp(false);
        let copy = Doc {
            kind: self.kind,
            doc: forked,
        };
        copy.configure();
        Ok(copy)
    }

    /// Makes the document's content what it was at `to` by writing new operations on top of the
    /// current history, in **one commit**: nothing is rewritten, the versions before stay, and the
    /// result merges and syncs like any edit. A document that already reads as it did at `to`
    /// gets no operation (its [`Doc::frontiers`] stay). `InvalidVersion` when `to` names
    /// operations the history does not have.
    pub fn revert_to(&self, to: &Frontiers) -> Result<(), DocError> {
        if to.is_empty() {
            // The empty version is "before everything": reverting to it would blank the document.
            return Err(DocError::InvalidVersion("the version id is empty".into()));
        }
        self.check_known(to)?;
        self.doc
            .revert_to(&to.0)
            .map_err(|e| DocError::Engine(e.to_string()))?;
        self.doc.commit();
        Ok(())
    }

    /// The runs of history that no session in `covered` accounts for, **newest first** (the
    /// undated versions of ADR-0057, section 2.3).
    ///
    /// A session covers the operations in its `end` version that are not in its `base`. Every
    /// other operation of the history belongs to a run: the operations are taken in the order of
    /// the history (by Lamport timestamp), and consecutive ones of one peer form a run; a
    /// covered operation, or one of another peer, ends it. A span whose versions the history
    /// does not have is ignored (its operations stay in their runs).
    pub fn runs_outside(&self, covered: &[Span]) -> Result<Vec<Run>, DocError> {
        self.doc.commit();
        let covered = self.covered_intervals(covered);
        let mut pieces: Vec<Piece> = Vec::new();
        for (peer, end) in self.doc.oplog_vv().iter() {
            let mut counter: i32 = 0;
            while counter < *end {
                let Some(change) = self.doc.get_change(ID::new(*peer, counter)) else {
                    break;
                };
                let len = i32::try_from(change.len).unwrap_or(i32::MAX);
                if len <= 0 {
                    break;
                }
                let first = change.id.counter;
                let stop = first.saturating_add(len);
                split_change(
                    *peer,
                    first,
                    stop,
                    u64::from(change.lamport),
                    covered.get(peer).map_or(&[], Vec::as_slice),
                    &mut pieces,
                );
                counter = stop;
            }
        }
        // Oldest first: by the Lamport timestamp of the piece's last operation (a piece's
        // operations follow everything it depends on), ties by peer then position.
        pieces.sort_unstable_by_key(|p| (p.lamport_last, p.peer, p.start));
        let mut runs: Vec<Run> = Vec::new();
        let mut open: Option<(PeerID, i32, u64, usize)> = None;
        for piece in &pieces {
            if piece.covered {
                runs.extend(close(open.take()));
                continue;
            }
            let operations = usize::try_from(piece.end - piece.start).unwrap_or(0);
            open = match open.take() {
                Some((peer, _, _, ops)) if peer == piece.peer => {
                    Some((peer, piece.end, piece.lamport_last, ops + operations))
                }
                previous => {
                    runs.extend(close(previous));
                    Some((piece.peer, piece.end, piece.lamport_last, operations))
                }
            };
        }
        runs.extend(close(open));
        runs.reverse();
        Ok(runs)
    }

    /// The counter ranges `[start, end)` per peer that the sessions cover, sorted and merged.
    fn covered_intervals(&self, spans: &[Span]) -> HashMap<PeerID, Vec<(i32, i32)>> {
        let mut covered: HashMap<PeerID, Vec<(i32, i32)>> = HashMap::new();
        for span in spans {
            let Some(end) = self.doc.frontiers_to_vv(&span.end.0) else {
                continue;
            };
            let Some(base) = (if span.base.is_empty() {
                Some(loro::VersionVector::default())
            } else {
                self.doc.frontiers_to_vv(&span.base.0)
            }) else {
                continue;
            };
            for (peer, stop) in end.iter() {
                let start = base.get(peer).copied().unwrap_or(0);
                if *stop > start {
                    covered.entry(*peer).or_default().push((start, *stop));
                }
            }
        }
        for intervals in covered.values_mut() {
            intervals.sort_unstable();
            let mut merged: Vec<(i32, i32)> = Vec::with_capacity(intervals.len());
            for (start, end) in intervals.drain(..) {
                match merged.last_mut() {
                    Some(last) if start <= last.1 => last.1 = last.1.max(end),
                    _ => merged.push((start, end)),
                }
            }
            *intervals = merged;
        }
        covered
    }
}

/// The run an open stretch `(peer, end, lamport of its last operation, operations)` makes.
fn close(open: Option<(PeerID, i32, u64, usize)>) -> Option<Run> {
    let (peer, end, lamport, operations) = open?;
    Some(Run {
        version: Frontiers::of_operation(peer, end - 1),
        peer,
        lamport,
        operations,
    })
}

/// Cuts the change of `peer` over the counters `[first, stop)` (its first operation has Lamport
/// timestamp `lamport`) into the stretches the sorted, merged `covered` intervals leave covered
/// and uncovered.
fn split_change(
    peer: PeerID,
    first: i32,
    stop: i32,
    lamport: u64,
    covered: &[(i32, i32)],
    out: &mut Vec<Piece>,
) {
    let mut push = |start: i32, end: i32, covered: bool| {
        if end > start {
            let last = u64::try_from(end - 1 - first).unwrap_or(0);
            out.push(Piece {
                peer,
                start,
                end,
                lamport_last: lamport + last,
                covered,
            });
        }
    };
    let mut position = first;
    for &(start, end) in covered {
        if end <= position {
            continue;
        }
        if start >= stop {
            break;
        }
        push(position, start.min(stop), false);
        let from = start.max(position);
        let to = end.min(stop);
        push(from, to, true);
        position = to;
    }
    push(position, stop, false);
}
