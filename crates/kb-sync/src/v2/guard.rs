//! What a v2 server decides about a shared feed's membership, once, for `MemoryRemote` and `kb-server`
//! alike (M13, ADR-0043, ADR-0045; `docs/development/milestone-13.md` 1.1 "Fold decisions (A)" and 1.3).
//!
//! The fold of `kb_keys::membership` is the rule book every client also runs. This module wraps it
//! with the parts only the server has:
//!
//! - the **compare-and-swap** (`seq == head_seq + 1` and `prev` equal to the head hash);
//! - the **server clock**: a `Join` of an expired invite is refused (clients cannot read that clock
//!   and do not re-check), an `Invite` may not expire more than `INVITE_MAX_DAYS` after the server's
//!   now;
//! - **inactive devices** refused at append time (an entry signed by a revoked device);
//! - the **Rekey rate limit** (`REKEYS_PER_ACCOUNT_PER_FEED_PER_DAY`);
//! - the **access list** derived from the folded log ([`Access`]): members with their role, removed
//!   or departed accounts with the cursor of their `Remove` or `Leave`;
//! - **`pending_rekey`**: a `Leave` sets it (the fold knows), and so does a device revocation in a
//!   member's device log that no `Rekey` covers yet. The second is derived from the logs (never
//!   stored): the member's log has a `DeviceRevoked` entry beyond the position the space key was last
//!   boxed for (`Timeline::head_seq`) and beyond the position the account joined at.
//!
//! [`FeedRules`] is a plain value: [`FeedRules::apply_batch`] returns the new rules and leaves `self`
//! alone, so a server applies a batch to a copy and commits it only when everything else (quota,
//! storage) succeeded. A batch is atomic by construction.

use std::collections::BTreeMap;

use kb_keys::log::LogEntryBody;
use kb_keys::membership::LogSource;
use kb_keys::membership::{
    Applied, ChainHead, EntryBody, EntryKind, FoldRule, MemberRecord, MembershipEntry,
    MembershipState, Timeline, rules,
};
use kb_types::sharing::{INVITE_MAX_DAYS, REKEYS_PER_ACCOUNT_PER_FEED_PER_DAY};
use kb_types::{AccountId, FeedId, MemberRole};

use crate::SyncError;

/// Milliseconds in a day.
const DAY_MS: i64 = 86_400_000;

/// The session account's standing in a shared feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// A current member with this role.
    Member(MemberRole),
    /// Removed or departed: the cursor of the `Remove` or `Leave` entry. Reads the membership log and
    /// the feed up to that cursor and nothing else.
    Removed {
        /// The feed cursor of the entry that ended the membership.
        cursor: u64,
    },
    /// Never a member (or the feed is unknown): the server answers as for an unknown feed.
    Stranger,
}

impl Access {
    /// The role of a current member.
    #[must_use]
    pub const fn role(self) -> Option<MemberRole> {
        match self {
            Self::Member(role) => Some(role),
            _ => None,
        }
    }

    /// The role if it is at least `min`: a stranger or a removed account is
    /// [`SyncError::NotFound`], a role that is too low [`SyncError::Forbidden`].
    pub fn require(self, min: MemberRole) -> Result<MemberRole, SyncError> {
        match self {
            Self::Member(role) if role >= min => Ok(role),
            Self::Member(_) => Err(SyncError::Forbidden),
            Self::Removed { .. } | Self::Stranger => Err(SyncError::NotFound),
        }
    }
}

/// Why a membership batch was refused. `index` is the position in the batch (0 for a failed
/// compare-and-swap).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchError {
    /// The entry of the batch.
    pub index: usize,
    /// What went wrong.
    pub kind: BatchErrorKind,
}

/// The categories of [`BatchError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchErrorKind {
    /// The first entry does not continue the log head: another writer appended first.
    Conflict,
    /// A device log the entry names is unknown or too short (the client publishes it first).
    UnknownLog,
    /// A signature (of the entry, or the `Join` proof's carrier) does not verify.
    Authentication,
    /// The signer's role does not allow the entry (not a member, not an Owner), or the signing device
    /// is not active.
    Role,
    /// The space was closed.
    Closed,
    /// A `Join` names an invite that is unknown, used, revoked or expired.
    DeadInvite,
    /// The signer made `REKEYS_PER_ACCOUNT_PER_FEED_PER_DAY` Rekeys in the last day.
    RateLimited,
    /// A malformed or rule-breaking entry; the text names the rule, never data.
    Invalid(&'static str),
}

impl BatchError {
    /// The [`SyncError`] the client sees. `member` is whether the session's account is a current
    /// member: a stranger who posts a `Join` learns only `Conflict` (the log moved: open the invite
    /// again) and otherwise `NotFound`, the one answer of every dead invite.
    #[must_use]
    pub fn sync_error(&self, member: bool) -> SyncError {
        if !member {
            return match self.kind {
                BatchErrorKind::Conflict | BatchErrorKind::UnknownLog => SyncError::Conflict(
                    "the membership log moved or a device log is missing; open the invite again"
                        .to_owned(),
                ),
                _ => SyncError::NotFound,
            };
        }
        match self.kind {
            BatchErrorKind::Conflict => SyncError::Conflict(
                "another membership entry was appended first; read the log and retry".to_owned(),
            ),
            BatchErrorKind::UnknownLog => SyncError::Conflict(
                "a device log the entry names is not on the server yet".to_owned(),
            ),
            BatchErrorKind::Authentication => SyncError::Authentication,
            BatchErrorKind::Role => SyncError::Forbidden,
            BatchErrorKind::Closed => SyncError::Closed,
            BatchErrorKind::DeadInvite => SyncError::Protocol("the invite is not open".to_owned()),
            BatchErrorKind::RateLimited => {
                SyncError::Unavailable("too many rekeys; try again later".to_owned())
            }
            BatchErrorKind::Invalid(rule) => {
                SyncError::Protocol(format!("the membership entry breaks a rule: {rule}"))
            }
        }
    }
}

/// What a batch is judged against besides the rules.
pub struct BatchContext<'a> {
    /// The server's clock, Unix ms.
    pub now_ms: i64,
    /// The server's copy of the device logs.
    pub logs: &'a dyn LogSource,
    /// Serve an expired invite as if it were open (the adversarial mode of the reference remote):
    /// the server clock is not consulted for `Join`s. Honest servers pass `false`.
    pub serve_expired: bool,
}

/// The server's whole view of one shared feed's membership: the fold's [`Timeline`] plus what the
/// access list and the limits need. Cheap to clone (the log of one space is short).
#[derive(Clone, Debug)]
pub struct FeedRules {
    timeline: Timeline,
    /// Where each account that left or was removed stood in the feed at that moment (the cursor of
    /// the entry); an account that joins again is no longer here.
    departed: BTreeMap<AccountId, u64>,
    /// The device log position each member joined at (`Join.log.seq`); the creator has none (its
    /// `Create` names its head).
    join_seq: BTreeMap<AccountId, u64>,
    /// The server time of every `Rekey`, by signer, for the rate limit.
    rekeys: Vec<(AccountId, i64)>,
}

impl FeedRules {
    /// The rules of an empty feed: the next entry must be a `Create`.
    #[must_use]
    pub fn new(feed: FeedId) -> Self {
        Self {
            timeline: Timeline::new(feed),
            departed: BTreeMap::new(),
            join_seq: BTreeMap::new(),
            rekeys: Vec::new(),
        }
    }

    /// The folded state.
    #[must_use]
    pub fn state(&self) -> &MembershipState {
        self.timeline.state()
    }

    /// The fold's timeline (who was what at which cursor).
    #[must_use]
    pub fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    /// The position of the last membership entry; `None` before the `Create`.
    #[must_use]
    pub fn head(&self) -> Option<ChainHead> {
        self.timeline.state().head
    }

    /// The space key epoch in force (0 before the `Create`).
    #[must_use]
    pub fn epoch(&self) -> u32 {
        self.timeline.state().epoch
    }

    /// Whether an Owner closed the space.
    #[must_use]
    pub fn closed(&self) -> bool {
        self.timeline.state().closed.is_some()
    }

    /// The current members, in join order.
    #[must_use]
    pub fn members(&self) -> &[MemberRecord] {
        &self.timeline.state().members
    }

    /// The accounts that left or were removed, with the cursor of that entry.
    #[must_use]
    pub fn departed(&self) -> &BTreeMap<AccountId, u64> {
        &self.departed
    }

    /// The standing of `account` in this feed.
    #[must_use]
    pub fn access(&self, account: AccountId) -> Access {
        if let Some(role) = self.timeline.state().role_of(account) {
            Access::Member(role)
        } else if let Some(cursor) = self.departed.get(&account) {
            Access::Removed { cursor: *cursor }
        } else {
            Access::Stranger
        }
    }

    /// The earliest-joined remaining Owner: the billing account when the creator is gone
    /// (ADR-0045 point 6).
    #[must_use]
    pub fn billing_successor(&self) -> Option<AccountId> {
        self.members()
            .iter()
            .find(|member| member.role.is_owner())
            .map(|member| member.account)
    }

    /// Whether content appends wait for a `Rekey`: a `Leave` is not covered yet, or a member's
    /// device log holds a revocation that no `Rekey` (and no later `Join`) covers.
    #[must_use]
    pub fn pending_rekey(&self, logs: &dyn LogSource) -> bool {
        if self.timeline.state().pending_rekey {
            return true;
        }
        self.members().iter().any(|member| {
            let covered = self
                .timeline
                .head_seq(member.account)
                .into_iter()
                .chain(self.join_seq.get(&member.account).copied())
                .max();
            let Some(log) = logs.device_log(member.account) else {
                return false;
            };
            log.entries().iter().any(|entry| {
                matches!(entry.body, LogEntryBody::DeviceRevoked { .. })
                    && covered.is_none_or(|covered| entry.seq > covered)
            })
        })
    }

    /// Rebuilds the rules from a stored membership log: `(feed cursor, server arrival time, entry
    /// bytes)` in log order. Only the fold runs (the server-only checks were made when the entries
    /// were accepted); the arrival times feed the Rekey rate limit.
    pub fn rebuild<'a>(
        feed: FeedId,
        items: impl IntoIterator<Item = (u64, i64, &'a [u8])>,
        logs: &dyn LogSource,
    ) -> Result<Self, BatchError> {
        let mut built = Self::new(feed);
        for (index, (cursor, at_ms, bytes)) in items.into_iter().enumerate() {
            let entry = MembershipEntry::decode(bytes).map_err(|_| BatchError {
                index,
                kind: BatchErrorKind::Invalid("encoding"),
            })?;
            let applied = built
                .timeline
                .apply(cursor, &entry, logs)
                .map_err(|error| fold_error(index, error.rule))?;
            built.note(&entry, &applied, cursor, at_ms);
        }
        Ok(built)
    }

    /// Applies a batch of membership entries found at the feed cursors given (one per entry,
    /// non-decreasing). Answers the new rules; `self` is untouched, so the caller commits the batch
    /// atomically by replacing its rules with the answer.
    ///
    /// Checks, in this order for each entry: the compare-and-swap (the first entry only), the
    /// signing device is active, the server-clock rules, then the fold.
    pub fn apply_batch(
        &self,
        entries: &[(u64, MembershipEntry)],
        ctx: &BatchContext<'_>,
    ) -> Result<Self, BatchError> {
        let mut next = self.clone();
        for (index, (cursor, entry)) in entries.iter().enumerate() {
            let fail = |kind| BatchError { index, kind };
            if index == 0 {
                next.check_continues(entry).map_err(fail)?;
            }
            next.server_checks(entry, ctx).map_err(fail)?;
            let applied = next
                .timeline
                .apply(*cursor, entry, ctx.logs)
                .map_err(|error| fold_error(index, error.rule))?;
            next.note(entry, &applied, *cursor, ctx.now_ms);
        }
        Ok(next)
    }

    /// The compare-and-swap: the entry continues the head (or is the `Create` of an empty log).
    fn check_continues(&self, entry: &MembershipEntry) -> Result<(), BatchErrorKind> {
        let header = &entry.header;
        let continues = match self.head() {
            None => header.seq == 0 && header.prev == [0u8; 32],
            Some(head) => head.seq.checked_add(1) == Some(header.seq) && header.prev == head.hash,
        };
        if continues {
            Ok(())
        } else {
            Err(BatchErrorKind::Conflict)
        }
    }

    /// The checks only the server makes (`milestone-13.md`, "What the server checks itself").
    fn server_checks(
        &self,
        entry: &MembershipEntry,
        ctx: &BatchContext<'_>,
    ) -> Result<(), BatchErrorKind> {
        let header = &entry.header;
        // An entry signed by a device that is not active in the signer's current log.
        let log = ctx
            .logs
            .device_log(header.account)
            .ok_or(BatchErrorKind::UnknownLog)?;
        let last = log
            .entries()
            .last()
            .map(|entry| entry.seq)
            .ok_or(BatchErrorKind::UnknownLog)?;
        let active = log
            .active_devices_at(last)
            .ok_or(BatchErrorKind::UnknownLog)?;
        if !active.iter().any(|(device, _)| *device == header.device) {
            return Err(BatchErrorKind::Role);
        }
        match &header.body {
            EntryBody::Invite { expires_at, .. } => {
                let horizon = ctx
                    .now_ms
                    .saturating_add(i64::from(INVITE_MAX_DAYS).saturating_mul(DAY_MS));
                if *expires_at > horizon {
                    return Err(BatchErrorKind::Invalid(rules::INVITE_EXPIRY_TOO_FAR));
                }
            }
            EntryBody::Join { invite_id, .. } => {
                if self
                    .timeline
                    .state()
                    .invite(*invite_id)
                    .is_some_and(|invite| !ctx.serve_expired && invite.is_expired(ctx.now_ms))
                {
                    return Err(BatchErrorKind::DeadInvite);
                }
            }
            EntryBody::Rekey(_) => {
                let since = ctx.now_ms.saturating_sub(DAY_MS);
                let recent = self
                    .rekeys
                    .iter()
                    .filter(|(account, at)| *account == header.account && *at > since)
                    .count();
                if recent >= usize::try_from(REKEYS_PER_ACCOUNT_PER_FEED_PER_DAY).unwrap_or(20) {
                    return Err(BatchErrorKind::RateLimited);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Records what an applied entry did beyond the fold's own state.
    fn note(&mut self, entry: &MembershipEntry, applied: &Applied, cursor: u64, at_ms: i64) {
        match applied.kind {
            EntryKind::Join => {
                self.departed.remove(&applied.signer);
                if let EntryBody::Join { log, .. } = &entry.header.body {
                    self.join_seq.insert(applied.signer, log.seq);
                }
            }
            EntryKind::Remove => {
                if let Some(target) = applied.subject {
                    self.departed.insert(target, cursor);
                    self.join_seq.remove(&target);
                }
            }
            EntryKind::Leave => {
                self.departed.insert(applied.signer, cursor);
                self.join_seq.remove(&applied.signer);
            }
            EntryKind::Rekey => self.rekeys.push((applied.signer, at_ms)),
            _ => {}
        }
    }
}

/// Maps a fold rule to the batch error kind.
fn fold_error(index: usize, rule: FoldRule) -> BatchError {
    let kind = match rule {
        FoldRule::Encoding => BatchErrorKind::Invalid("encoding"),
        FoldRule::Chain => BatchErrorKind::Invalid("chain"),
        FoldRule::Signature => BatchErrorKind::Authentication,
        FoldRule::UnknownLog => BatchErrorKind::UnknownLog,
        FoldRule::Completeness => BatchErrorKind::Invalid("completeness"),
        FoldRule::Heads => BatchErrorKind::Invalid("heads"),
        FoldRule::Rule(name) => match name {
            rules::SIGNER_NOT_MEMBER | rules::SIGNER_NOT_OWNER => BatchErrorKind::Role,
            rules::CLOSED => BatchErrorKind::Closed,
            rules::INVITE_UNKNOWN | rules::INVITE_USED | rules::INVITE_REVOKED => {
                BatchErrorKind::DeadInvite
            }
            other => BatchErrorKind::Invalid(other),
        },
    };
    BatchError { index, kind }
}
