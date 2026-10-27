//! The invitee's side after the invite was found (`docs/security/sharing.md` section 7, steps 5 and 6;
//! ADR-0046): verifying the membership log the server answers `invites/{id}/open` with, adopting the
//! feed (registering it, holding the space key, folding the log), the `Join`, and the roll-back when the
//! session ends before the server took the `Join`.
//!
//! What verifying checks: every entry decodes, belongs to the feed and folds (signatures by devices
//! of the signers' own logs, which the host fetched); the invite is in the log, open, written by an
//! Owner, with the public key the secret derives and bound to this account when the code flow made
//! it; the log is not closed and this account is not a member; the key of the bundle matches the
//! commitment of its epoch; in the code flow the log holds the entry the bundle's head names (and the
//! `Invite` is the inviter's).

use std::collections::{BTreeMap, BTreeSet};

use kb_keys::invite::{
    InviteSecret, invite_id as derive_invite_id, invite_signing_key, open_link_bundle,
    sign_join_proof,
};
use kb_keys::membership::{
    ChainHead, EntryBody, FoldRule, InviteRecordStatus, MembershipEntry, Private, Timeline,
};
use kb_keys::safety::{genesis_hash, safety_number};
use kb_keys::space_keys::{SpaceKey, membership_meta_key};
use kb_storage::FeedQueries;
use kb_sync::v2::{InviteOpened, MembershipHead, MembershipPage};
use kb_types::{
    AccountId, ApiError, Event, FeedId, InviteFlow, InviteId, InviteSessionState, MemberRole,
};

use super::{InviteSession, UNVERIFIED_MESSAGE};
use crate::membership::{LogsView, entry_accounts, fallback_name};
use crate::{Core, map_storage_error};

/// What verifying an opened invite found.
#[derive(Clone)]
pub(crate) struct Verification {
    pub(crate) feed: FeedId,
    pub(crate) epoch: u32,
    pub(crate) key: SpaceKey,
    pub(crate) role: MemberRole,
    pub(crate) expires_at: i64,
    pub(crate) space_name: Option<String>,
    pub(crate) inviter: AccountId,
    pub(crate) inviter_name: String,
    pub(crate) invite_id: InviteId,
}

/// The result of one verification attempt.
enum Outcome {
    Verified(Box<Verification>),
    /// A device log is missing: the core asked for it; try again once the host brought it.
    NeedsLogs,
    /// The log or the invite is not what it should be; the text is safe to show.
    Invalid(&'static str),
}

const NOT_VERIFIED: &str = "this invite could not be verified";

impl Core {
    /// The code flow's bundle arrived (`state: Joining`).
    pub(super) fn invitee_takes_bundle(
        &mut self,
        session: &mut InviteSession,
        bundle: kb_keys::invite::CodeBundle,
    ) -> Result<(), ApiError> {
        let me = self.me()?;
        if bundle.inviter == me || Some(bundle.feed_id) == self.personal_feed()? {
            return Err(ApiError::crypto(UNVERIFIED_MESSAGE));
        }
        if self
            .db
            .sync_feed_get(bundle.feed_id)
            .map_err(map_storage_error)?
            .is_some()
        {
            // A removed account that is invited again keeps its archive: it comes back to life. A
            // live membership is not joined twice.
            if self.feed_archived(bundle.feed_id)?.is_none() {
                session.fail("you are already a member of this space");
                return Ok(());
            }
        } else if self.docs.structures.contains_key(&bundle.space_id) {
            session.fail("this device already has a space with the same id");
            return Ok(());
        }
        session.space_id = Some(bundle.space_id);
        session.feed = Some(bundle.feed_id);
        session.secret = Some(bundle.secret.clone());
        session.space_name = Some(bundle.space_name.clone());
        session.bundle = Some(bundle);
        session.state = InviteSessionState::Joining;
        Ok(())
    }

    /// The server's answer to `invites/{id}/open`.
    pub(super) fn invitee_opened(
        &mut self,
        session: &mut InviteSession,
        opened: &InviteOpened,
    ) -> Result<(), ApiError> {
        if !session.is_active() || session.side != kb_types::InviteSide::Invitee {
            return Ok(());
        }
        let Some(secret) = session.secret.clone() else {
            return Err(ApiError::invalid("state", "no invite is being opened"));
        };
        if session.flow == InviteFlow::Code && session.bundle.is_none() {
            return Err(ApiError::invalid(
                "state",
                "the invite has not been handed over yet",
            ));
        }
        if session.verification.is_some() && session.opened.as_ref() == Some(opened) {
            return Ok(());
        }
        match self.verify_opened(session, &secret, opened)? {
            Outcome::NeedsLogs => Ok(()),
            Outcome::Invalid(message) => {
                // The session ends here, with a reason the dialog shows; nothing more to do for the host.
                self.invite_end(session, InviteSessionState::Failed, true);
                session.error = Some(message.to_owned());
                Ok(())
            }
            Outcome::Verified(found) => {
                session.feed = Some(opened.feed);
                session.role = Some(found.role);
                session.expires_at = Some(found.expires_at);
                if found.space_name.is_some() {
                    session.space_name.clone_from(&found.space_name);
                }
                session.inviter_name = Some(found.inviter_name.clone());
                session.peer_name = Some(found.inviter_name.clone());
                session.peer_safety_number = self.safety_number_with(found.inviter);
                session.opened = Some(opened.clone());
                session.verification = Some((*found).clone());
                match session.flow {
                    InviteFlow::Link => session.state = InviteSessionState::Previewing,
                    InviteFlow::Code => self.invitee_adopt(session)?,
                }
                Ok(())
            }
        }
    }

    /// The safety number between this account and a co-member whose log is held.
    fn safety_number_with(&self, account: AccountId) -> Option<String> {
        let own = self.security.log.as_ref().map(genesis_hash)?;
        let other = self.shared.logs.get(&account).map(genesis_hash)?;
        Some(safety_number(&own, &other))
    }

    /// Verifies the log of an opened invite against the invite's secret (and the bundle of the code
    /// flow); see the module documentation.
    fn verify_opened(
        &mut self,
        session: &InviteSession,
        secret: &InviteSecret,
        opened: &InviteOpened,
    ) -> Result<Outcome, ApiError> {
        let me = self.me()?;
        let feed = opened.feed;
        let mut entries: Vec<MembershipEntry> = Vec::with_capacity(opened.log.len());
        for item in &opened.log {
            match MembershipEntry::decode(&item.bytes) {
                Ok(entry) if entry.header.feed == feed => entries.push(entry),
                _ => return Ok(Outcome::Invalid(NOT_VERIFIED)),
            }
        }
        if entries.is_empty()
            || session
                .bundle
                .as_ref()
                .is_some_and(|bundle| bundle.feed_id != feed)
        {
            return Ok(Outcome::Invalid(NOT_VERIFIED));
        }
        // Every signer's device log must be here before the fold can judge.
        let mut named: BTreeSet<AccountId> = BTreeSet::new();
        for entry in &entries {
            named.extend(entry_accounts(entry));
        }
        named.remove(&me);
        let missing: Vec<AccountId> = named
            .iter()
            .copied()
            .filter(|account| !self.shared.logs.contains_key(account))
            .collect();
        if !missing.is_empty() {
            self.shared.wanted_logs.extend(missing);
            return Ok(Outcome::NeedsLogs);
        }
        let mut timeline = Timeline::new(feed);
        let folded = {
            let view = LogsView {
                me,
                own: self.security.log.as_ref(),
                foreign: &self.shared.logs,
            };
            let mut verdict = Ok(());
            for (item, entry) in opened.log.iter().zip(&entries) {
                if let Err(error) = timeline.apply(item.cursor, entry, &view) {
                    verdict = Err(error.rule);
                    break;
                }
            }
            verdict
        };
        if let Err(rule) = folded {
            let stale = match rule {
                FoldRule::UnknownLog | FoldRule::Heads => true,
                // A log fetched before a signer's device was added: ask again once.
                FoldRule::Signature => named
                    .iter()
                    .any(|account| !self.shared.fresh_logs.contains(account)),
                _ => false,
            };
            if stale {
                self.shared.wanted_logs.extend(named);
                return Ok(Outcome::NeedsLogs);
            }
            tracing::warn!(?rule, "an invite's membership log does not fold");
            return Ok(Outcome::Invalid(NOT_VERIFIED));
        }
        let state = timeline.state();
        if state.closed.is_some() {
            return Ok(Outcome::Invalid(NOT_VERIFIED));
        }
        if state.member(me).is_some() {
            return Ok(Outcome::Invalid("you are already a member of this space"));
        }
        let invite_id = derive_invite_id(secret);
        let signing = invite_signing_key(secret);
        let Some(record) = state.invite(invite_id) else {
            return Ok(Outcome::Invalid(NOT_VERIFIED));
        };
        let bound_ok = match (record.bound_account, session.flow) {
            (Some(bound), _) => bound == me,
            (None, InviteFlow::Link) => true,
            (None, InviteFlow::Code) => false,
        };
        if !matches!(record.status, InviteRecordStatus::Open)
            || record.invite_pub != signing.verifying_key()
            || state.role_of(record.created_by) != Some(MemberRole::Owner)
            || record.role.is_owner()
            || !bound_ok
        {
            return Ok(Outcome::Invalid(NOT_VERIFIED));
        }
        let inviter = record.created_by;
        // The key: from the bundle of the code flow (whose head the log must hold), or from the
        // sealed bundle of the invite itself.
        let (epoch, key, space_name, link_inviter_name) = match &session.bundle {
            Some(bundle) => {
                let head_entry = usize::try_from(bundle.membership_head.seq)
                    .ok()
                    .and_then(|index| entries.get(index));
                let head_ok =
                    head_entry.is_some_and(|entry| entry.hash() == bundle.membership_head.hash);
                if bundle.inviter != inviter || !head_ok {
                    return Ok(Outcome::Invalid(NOT_VERIFIED));
                }
                (
                    bundle.epoch,
                    bundle.space_key.clone(),
                    Some(bundle.space_name.clone()),
                    None,
                )
            }
            None => {
                let Some(enc) = record.enc_bundle.as_ref() else {
                    return Ok(Outcome::Invalid(NOT_VERIFIED));
                };
                let Ok(link) = open_link_bundle(enc, secret) else {
                    return Ok(Outcome::Invalid(NOT_VERIFIED));
                };
                (
                    link.epoch,
                    link.space_key,
                    Some(link.space_name),
                    Some(link.inviter_name),
                )
            }
        };
        if commitments(&entries).get(&epoch) != Some(&key.commitment(feed, epoch)) {
            return Ok(Outcome::Invalid(NOT_VERIFIED));
        }
        let inviter_name =
            link_inviter_name.unwrap_or_else(|| name_in_log(&entries, inviter, epoch, &key, feed));
        Ok(Outcome::Verified(Box::new(Verification {
            feed,
            epoch,
            key,
            role: record.role,
            expires_at: record.expires_at,
            space_name,
            inviter,
            inviter_name,
            invite_id,
        })))
    }

    /// The preview was accepted (link flow): the feed is adopted and the session is `Joining`.
    pub(super) fn invitee_accepts_preview(
        &mut self,
        session: &mut InviteSession,
    ) -> Result<(), ApiError> {
        if session.verification.is_none() {
            return Err(ApiError::invalid("invite", "there is nothing to confirm"));
        }
        self.invitee_adopt(session)?;
        if session.is_active() {
            session.state = InviteSessionState::Joining;
        }
        Ok(())
    }

    /// Takes the feed over: sync is pointed at the invite's server, an archive of the same feed comes
    /// back to life, the feed is registered, the space key held, and the log folded (without the
    /// role events of a log that replays an earlier membership of this account).
    pub(super) fn invitee_adopt(&mut self, session: &mut InviteSession) -> Result<(), ApiError> {
        if session.adopted {
            return Ok(());
        }
        let (Some(found), Some(opened)) = (session.verification.clone(), session.opened.clone())
        else {
            return Err(ApiError::invalid("state", "the invite is not verified yet"));
        };
        let feed = found.feed;
        if self
            .db
            .sync_feed_get(feed)
            .map_err(map_storage_error)?
            .is_some()
        {
            if self.feed_archived(feed)?.is_none() {
                self.invite_end(session, InviteSessionState::Failed, true);
                session.error = Some("you are already a member of this space".to_owned());
                return Ok(());
            }
            self.feed_revive(feed)?;
        }
        if self.sync_server_url()?.is_none() {
            self.sync_configure(Some(&session.server_url), None)?;
            session.configured_sync = true;
        }
        let space = session.space_id.or(self.feed_space(feed)?);
        self.feed_register_shared(feed, space)?;
        self.feed_store_sk(feed, found.epoch, &found.key)?;
        let Some(last) = opened.log.last() else {
            return Err(ApiError::crypto(NOT_VERIFIED));
        };
        let head: MembershipHead = MembershipEntry::decode(&last.bytes)
            .map(|entry| ChainHead {
                seq: entry.header.seq,
                hash: entry.hash(),
            })
            .map_err(crate::map_keys_error)?;
        let page = MembershipPage {
            entries: opened.log.clone(),
            head,
            more: false,
        };
        let events_before = self.events.len();
        self.shared.adopting = Some(feed);
        let result = self.sync_membership_fetched(feed, &page);
        self.shared.adopting = None;
        result?;
        // The replay of an earlier membership of this account must not archive or alarm.
        self.events.truncate(events_before);
        let folded = self.shared.timelines.get(&feed).map_or(0, Timeline::len);
        if folded != opened.log.len() {
            self.invitee_rollback(session);
            return Err(ApiError::crypto(NOT_VERIFIED));
        }
        self.feed_derive_older_keys(feed)?;
        session.adopted = true;
        session.space_id = session.space_id.or(self.feed_space(feed)?);
        Ok(())
    }

    /// A feed this account had as an archive is a clean start again: its rows and settings go, the
    /// space and the keys stay.
    fn feed_revive(&mut self, feed: FeedId) -> Result<(), ApiError> {
        let space = self.feed_space(feed)?;
        self.feed_forget(feed)?;
        self.feed_register_shared(feed, space)
    }

    /// The `Join`: minted, stored as pending (the task posts it).
    pub(super) fn invitee_join(
        &mut self,
        session: &mut InviteSession,
    ) -> Result<Option<(FeedId, Vec<u8>)>, ApiError> {
        if !session.adopted || session.state != InviteSessionState::Joining {
            return Ok(None);
        }
        let (Some(found), Some(secret)) = (session.verification.clone(), session.secret.clone())
        else {
            return Ok(None);
        };
        if let Some(bytes) = &session.join_entry {
            return Ok(Some((found.feed, bytes.clone())));
        }
        let me = self.me()?;
        let log_head = {
            let log =
                self.security.log.as_ref().ok_or_else(|| {
                    ApiError::invalid("account", "create your recovery kit first")
                })?;
            let Some(last) = log.entries().last() else {
                return Err(ApiError::invalid("account", "the device log is empty"));
            };
            ChainHead {
                seq: last.seq,
                hash: last.hash(),
            }
        };
        let proof = sign_join_proof(
            &invite_signing_key(&secret),
            found.feed,
            found.invite_id,
            me,
            &log_head,
        );
        let entry = self.membership_mint(
            found.feed,
            EntryBody::Join {
                invite_id: found.invite_id,
                proof,
                account: me,
                log: log_head,
            },
            Private::Join {
                profile_name: self.invite_display_name()?,
            },
            Some((found.epoch, found.key.clone())),
        )?;
        let bytes = entry.encode();
        session.join_entry = Some(bytes.clone());
        Ok(Some((found.feed, bytes)))
    }

    /// The server took the `Join`: the membership is real. The space is written into the account
    /// document (the account's other devices find it there), the people are marked, sync goes.
    pub(super) fn invitee_joined(
        &mut self,
        session: &mut InviteSession,
        head: MembershipHead,
    ) -> Result<(), ApiError> {
        let (Some(found), true) = (session.verification.clone(), session.adopted) else {
            return Err(ApiError::invalid("state", "the invite was not joined"));
        };
        self.sync_membership_posted(found.feed, head)?;
        session.join_posted = true;
        self.fcfg_set(found.feed, "role", &found.role.to_byte().to_string())?;
        let now = self.clock.now_ms();
        self.ensure_account()?;
        let space = session.space_id.or(self.feed_space(found.feed)?);
        if let Some(account) = self.docs.account.as_ref() {
            if let Some(space) = space {
                account
                    .model
                    .set_shared_space(space, found.feed)
                    .map_err(crate::documents::map_model_error)?;
            }
            if session.flow == InviteFlow::Code {
                // The digits authenticated both accounts (ADR-0042).
                account
                    .model
                    .set_verified(found.inviter, Some(now))
                    .map_err(crate::documents::map_model_error)?;
            }
        }
        self.persist_account()?;
        session.verified = session.flow == InviteFlow::Code;
        session.state = InviteSessionState::Done;
        self.events.push(Event::SpacesChanged);
        if let Some(space_id) = space {
            self.push_event_once(Event::SpaceMembersChanged { space_id });
        }
        self.push_event_once(Event::PeopleChanged);
        self.push_event_once(Event::SyncRequested);
        self.push_event_once(Event::SyncChanged);
        tracing::info!("a shared space was joined");
        Ok(())
    }

    /// Undoes what the invitee did before the server took its `Join`: the adopted feed goes, the key
    /// records of a space this device never had, the sync configuration this session made.
    pub(super) fn invitee_rollback(&mut self, session: &mut InviteSession) {
        if session.join_posted {
            return;
        }
        if session.adopted
            && let Some(feed) = session.feed
        {
            let space = session.space_id;
            let had_space = space.is_some_and(|space| self.docs.structures.contains_key(&space));
            if let Err(error) = self.feed_forget(feed) {
                tracing::warn!(code = ?error.code, "an adopted feed could not be dropped");
            }
            if let Some(space) = space
                && !had_space
                && let Err(error) = self.keyring_forget_space(space)
            {
                tracing::warn!(code = ?error.code, "the keys of an unjoined space could not be dropped");
            }
            session.adopted = false;
            session.join_entry = None;
        }
        if session.configured_sync {
            if let Err(error) = self.sync_configure(None, None) {
                tracing::warn!(code = ?error.code, "sync could not be turned off again");
            }
            session.configured_sync = false;
        }
    }
}

/// The commitment each epoch of the log started with: the `Create` and every `Rekey` or `Remove`.
fn commitments(entries: &[MembershipEntry]) -> BTreeMap<u32, [u8; 32]> {
    let mut out = BTreeMap::new();
    for entry in entries {
        match &entry.header.body {
            EntryBody::Create {
                epoch,
                sk_commitment,
                ..
            } => {
                out.insert(*epoch, *sk_commitment);
            }
            EntryBody::Rekey(rekey) | EntryBody::Remove { rekey, .. } => {
                out.insert(rekey.new_epoch, rekey.sk_commitment);
            }
            _ => {}
        }
    }
    out
}

/// The latest name an account gave in entries sealed under the epoch of `key` (the only ones the
/// bundle opens), or a stand-in made of the account id.
fn name_in_log(
    entries: &[MembershipEntry],
    account: AccountId,
    epoch: u32,
    key: &SpaceKey,
    feed: FeedId,
) -> String {
    let meta_key = membership_meta_key(key, feed);
    entries
        .iter()
        .rev()
        .filter(|entry| entry.header.account == account && entry.header.meta_epoch == epoch)
        .find_map(|entry| match entry.open_private(&meta_key).ok()? {
            Private::Create { profile_name, .. } | Private::Join { profile_name } => {
                Some(profile_name)
            }
            Private::Profile { display_name } => Some(display_name),
            _ => None,
        })
        .unwrap_or_else(|| fallback_name(account))
}
