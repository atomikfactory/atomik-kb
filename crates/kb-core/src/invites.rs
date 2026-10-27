//! Invites to a shared space (M13 slice 1, ADR-0046, `docs/development/milestone-13.md` sections 1.1
//! and 1.5): the two session state machines (code flow and link flow, inviter and invitee), the
//! link invites, revocation, and the host methods `kb-net`'s `InviteTask` drives, mirroring the
//! pairing ones of `pairing.rs`: the shell's task opens the mailbox, posts what [`Core::invite_outbox`]
//! hands it and feeds back what it fetched; the user confirms through `InviteConfirm`.
//!
//! Mailbox sequences of the code flow (the opener is the inviter, the typist the invitee; as in
//! pairing the first three are in the clear): 0 offer (the commitment), 1 answer, 2 reveal, 3 the
//! invitee's sealed `InviteIdentity`, 4 the inviter's sealed `InviteBundle`; an abort takes the next
//! sequence. The invitee confirms first.
//!
//! **What the host does, in order** (the `InviteHost` of `kb-net` mirrors these one to one):
//!
//! - *Inviter, code flow:* `invite_outbox` (the offer), the mailbox is created, `invite_mailbox_opened`,
//!   `invite_posted(0)`; the task then posts what `invite_outbox` holds and hands every message of the
//!   invitee to `invite_receive`; after the inviter confirmed (`InviteConfirm`), the `Invite` entry goes
//!   to the server by compare-and-swap (`invite_membership_pending`, `membership_append`,
//!   `invite_membership_posted`), and only then does `invite_outbox` hold the bundle (message 4). The
//!   session is `Done` when the invitee's `Join` is folded (the sync driver's pull).
//! - *Invitee, code flow:* `invite_mailbox_id` (from the typed code), `invite_mailbox_opened`, the
//!   messages as above; when the bundle arrived (`state: Joining`): `invite_signup` (an account on the
//!   server), authenticate, `invite_open_request`, `invites/{id}/open`, `invite_opened` (verifies the
//!   log; while `sync_needed_logs` is not empty after it, fetch those logs, hand them to
//!   `sync_foreign_log_received` and call `invite_opened` again), then `invite_join`, the post,
//!   `invite_joined`.
//! - *Invitee, link flow:* `invite_open_request`, `invites/{id}/open`, `invite_opened` (the same
//!   loop; the state becomes `Previewing`); after `InviteConfirm`: `invite_signup`, authenticate,
//!   `invite_join`, the post, `invite_joined`.
//!
//! The session lives in memory only. A feed the invitee adopts before the `Join` is on the server is
//! rolled back when the session fails or is cancelled.

mod open;

use kb_keys::invite::{
    CodeBundle, InviteSecret, LinkBundle, invite_id as derive_invite_id, invite_signing_key,
    seal_link_bundle, sign_open, sign_signup,
};
use kb_keys::membership::{ChainHead, EntryBody, InviteRecordStatus, Private};
use kb_keys::pairing::{InviteIdentity, MessageKind, PairingCode, Session};
use kb_keys::safety::{genesis_hash, safety_number};
use kb_storage::FeedQueries;
use kb_sync::v2::{
    InviteOpenRequest, InviteOpened, InviteSignup, MembershipHead, PendingMembership,
};
use kb_types::sharing::{
    INVITE_CODE_TTL_SECONDS, INVITE_LINK_DEFAULT_DAYS, INVITE_MAX_DAYS, INVITE_TRANSCRIPT_DOMAIN,
};
use kb_types::{
    ApiError, Event, FeedId, InviteFlow, InviteId, InviteLinkInfo, InviteSessionInfo,
    InviteSessionState, InviteSide, MemberRole, MembersInfo, Request, Response, SpaceId,
};

use crate::recovery::validate_server_url;
use crate::{Core, map_keys_error, map_storage_error};

const SEQ_OFFER: u32 = 0;
const SEQ_ANSWER: u32 = 1;
const SEQ_REVEAL: u32 = 2;
const SEQ_IDENTITY: u32 = 3;
const SEQ_BUNDLE: u32 = 4;
const DAY_MS: i64 = 86_400_000;

/// What a message or bundle that fails to open ends the session with.
const UNVERIFIED_MESSAGE: &str = "the other device could not be verified";

/// The invite session this core runs, if any (either flow, either side).
pub(crate) struct InviteSession {
    pub(crate) flow: InviteFlow,
    pub(crate) side: InviteSide,
    pub(crate) state: InviteSessionState,
    pub(crate) server_url: String,
    pub(crate) mailbox_id: Option<[u8; 6]>,
    pub(crate) expires_at: Option<i64>,
    pub(crate) error: Option<String>,
    pub(crate) peer_name: Option<String>,
    pub(crate) peer_safety_number: Option<String>,
    pub(crate) space_name: Option<String>,
    pub(crate) inviter_name: Option<String>,
    pub(crate) role: Option<MemberRole>,
    pub(crate) verified: bool,
    /// The sealed channel of the code flow.
    pub(crate) channel: Option<Session>,
    /// Messages to post, by mailbox sequence.
    pub(crate) outbox: Vec<(u32, Vec<u8>)>,
    /// The sequence of this side's last message.
    pub(crate) last_seq: u32,
    /// The space (inviter: the one invited to; invitee, code flow: from the bundle).
    pub(crate) space_id: Option<SpaceId>,
    /// The feed (inviter: the space's; invitee: from the bundle or the open answer).
    pub(crate) feed: Option<FeedId>,
    /// The invite's secret `s` (inviter: the one the `Invite` was made with; invitee: from the bundle
    /// or the link).
    pub(crate) secret: Option<InviteSecret>,
    /// Inviter: the invitee's verified identity.
    pub(crate) peer: Option<InviteIdentity>,
    /// Inviter: the `seq` of the `Invite` entry and the bundle to send once it is on the server.
    pub(crate) invite_seq: Option<u64>,
    pub(crate) bundle: Option<CodeBundle>,
    pub(crate) bundle_released: bool,
    /// Invitee: the answer of `invites/{id}/open`, once received, and what verifying it found.
    pub(crate) opened: Option<InviteOpened>,
    pub(crate) verification: Option<open::Verification>,
    /// Invitee: the feed was adopted (registered, key stored, log folded).
    pub(crate) adopted: bool,
    /// Invitee: the `Join` entry minted, and whether the server took it.
    pub(crate) join_entry: Option<Vec<u8>>,
    pub(crate) join_posted: bool,
    /// Invitee: this session pointed sync at the invite's server (undone when it fails).
    pub(crate) configured_sync: bool,
    /// Inviter: a lost compare-and-swap may have dropped the `Invite` (written again when the log
    /// has caught up).
    pub(crate) remint: bool,
}

impl InviteSession {
    fn new(flow: InviteFlow, side: InviteSide, server_url: String) -> Self {
        Self {
            flow,
            side,
            state: InviteSessionState::Starting,
            server_url,
            mailbox_id: None,
            expires_at: None,
            error: None,
            peer_name: None,
            peer_safety_number: None,
            space_name: None,
            inviter_name: None,
            role: None,
            verified: false,
            channel: None,
            outbox: Vec::new(),
            last_seq: SEQ_OFFER,
            space_id: None,
            feed: None,
            secret: None,
            peer: None,
            invite_seq: None,
            bundle: None,
            bundle_released: false,
            opened: None,
            verification: None,
            adopted: false,
            join_entry: None,
            join_posted: false,
            configured_sync: false,
            remint: false,
        }
    }

    fn is_active(&self) -> bool {
        !matches!(
            self.state,
            InviteSessionState::Idle
                | InviteSessionState::Done
                | InviteSessionState::Failed
                | InviteSessionState::Cancelled
        )
    }

    fn fail(&mut self, message: &str) {
        self.state = InviteSessionState::Failed;
        self.error = Some(message.to_owned());
    }
}

// ----- links -----------------------------------------------------------------------------------

/// Percent-encodes a server address for the `server=` parameter of an app link (everything but the
/// unreserved characters).
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Reverses [`percent_encode`]; `None` for a malformed escape or text that is not UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let pair = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(pair, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The server and the secret of an invite link: `atomik-kb://invite?server=<url>#<secret>` or
/// `<server>/join#<secret>`. `InvalidInput` for `link` otherwise.
pub(crate) fn parse_invite_link(raw: &str) -> Result<(String, InviteSecret), ApiError> {
    let bad = || ApiError::invalid("link", "this is not an invite link");
    let text = raw.trim();
    let (head, fragment) = text.split_once('#').ok_or_else(bad)?;
    let secret = InviteSecret::from_base32(fragment).map_err(|_| bad())?;
    let lower = head.to_ascii_lowercase();
    let server = if let Some(rest) = lower.strip_prefix("atomik-kb://invite?") {
        let query = &head[head.len() - rest.len()..];
        let value = query
            .split('&')
            .find_map(|part| {
                part.strip_prefix("server=")
                    .or_else(|| part.strip_prefix("SERVER="))
            })
            .ok_or_else(bad)?;
        percent_decode(value).ok_or_else(bad)?
    } else if (lower.starts_with("http://") || lower.starts_with("https://"))
        && lower.ends_with("/join")
    {
        head[..head.len() - "/join".len()].to_owned()
    } else {
        return Err(bad());
    };
    let server = validate_server_url(&server).map_err(|_| bad())?;
    Ok((server, secret))
}

impl Core {
    /// Routes the invite requests: `InviteLinkCreate`, `InviteRevoke`, `InviteCodeStart`,
    /// `InviteCodeJoin`, `InviteLinkOpen`, `InviteConfirm`, `InviteCancel` and `InviteStatus`.
    pub(crate) fn dispatch_invites(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::InviteLinkCreate {
                space_id,
                role,
                expires_in_days,
            } => Ok(Response::InviteLink(self.invite_link_create(
                space_id,
                role,
                expires_in_days,
            )?)),
            Request::InviteRevoke {
                space_id,
                invite_id,
            } => Ok(Response::Members(self.invite_revoke(space_id, invite_id)?)),
            Request::InviteCodeStart { space_id, role } => {
                self.invite_code_start(space_id, role)?;
                Ok(Response::Invite(self.invite_info()?))
            }
            Request::InviteCodeJoin { server_url, code } => {
                self.invite_code_join(&server_url, &code)?;
                Ok(Response::Invite(self.invite_info()?))
            }
            Request::InviteLinkOpen { link } => {
                self.invite_link_open(&link)?;
                Ok(Response::Invite(self.invite_info()?))
            }
            Request::InviteConfirm => {
                self.invite_confirm()?;
                Ok(Response::Invite(self.invite_info()?))
            }
            Request::InviteCancel => {
                self.invite_cancel()?;
                Ok(Response::Invite(self.invite_info()?))
            }
            Request::InviteStatus => Ok(Response::Invite(self.invite_info()?)),
            other => Err(ApiError::internal(format!(
                "{} is not an invite request",
                other.kind()
            ))),
        }
    }

    fn invite_changed(&mut self) {
        self.push_event_once(Event::InviteChanged);
    }

    /// Runs `f` on the session with the rest of the core free to use; the session goes back whatever
    /// `f` answers.
    fn invite_with<T>(
        &mut self,
        f: impl FnOnce(&mut Self, &mut InviteSession) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        let Some(mut session) = self.invite.take() else {
            return Err(ApiError::not_found("no invite is running"));
        };
        let result = f(self, &mut session);
        self.invite = Some(session);
        result
    }

    // ----- requests -----------------------------------------------------------------------

    /// Writes an `Invite` entry with a fresh secret and answers the link once (Owner; the role is
    /// not Owner; default `INVITE_LINK_DEFAULT_DAYS`, at most `INVITE_MAX_DAYS`).
    pub(crate) fn invite_link_create(
        &mut self,
        space_id: SpaceId,
        role: MemberRole,
        expires_in_days: Option<u32>,
    ) -> Result<InviteLinkInfo, ApiError> {
        let row = self.shared_space_for_request(space_id)?;
        self.require_role(space_id, MemberRole::Owner)?;
        check_invite_role(role)?;
        let days = expires_in_days.unwrap_or(INVITE_LINK_DEFAULT_DAYS);
        if !(1..=INVITE_MAX_DAYS).contains(&days) {
            return Err(ApiError::invalid(
                "expiresInDays",
                format!("must be 1 to {INVITE_MAX_DAYS} days"),
            ));
        }
        let Some(server) = self.sync_server_url()? else {
            return Err(ApiError::invalid(
                "serverUrl",
                "turn sync on before inviting people",
            ));
        };
        self.check_invite_room(row.feed)?;
        let me = self.me()?;
        let epoch = self
            .tentative_state(row.feed)?
            .ok_or_else(|| ApiError::not_found("the shared space is not known here"))?
            .epoch;
        let key = self.feed_sk(row.feed, epoch)?.ok_or_else(|| {
            ApiError::crypto("this device does not hold the space key of the current epoch")
        })?;
        let space_name = self.structure(space_id)?.model.space().name;
        let bundle = LinkBundle {
            epoch,
            space_key: key,
            space_name,
            inviter_name: self
                .profile_name_of_account()
                .unwrap_or_else(|| self.member_name(row.feed, me)),
        };
        let secret = InviteSecret::generate(self.rng.as_ref());
        let invite_id = derive_invite_id(&secret);
        let signing = invite_signing_key(&secret);
        let enc_bundle = seal_link_bundle(&bundle, &secret, self.rng.as_ref());
        let expires_at = self
            .clock
            .now_ms()
            .saturating_add(i64::from(days).saturating_mul(DAY_MS));
        self.membership_mint(
            row.feed,
            EntryBody::Invite {
                invite_id,
                invite_pub: signing.verifying_key(),
                role,
                expires_at,
                bound_account: None,
                verified: false,
                enc_bundle: Some(enc_bundle),
            },
            Private::None,
            None,
        )?;
        self.push_event_once(Event::SpaceMembersChanged { space_id });
        self.push_event_once(Event::SyncRequested);
        Ok(InviteLinkInfo {
            invite_id,
            link: format!(
                "atomik-kb://invite?server={}#{}",
                percent_encode(&server),
                secret.to_base32()
            ),
            web_link: format!(
                "{}/join#{}",
                server.trim_end_matches('/'),
                secret.to_base32()
            ),
            role,
            expires_at,
        })
    }

    /// Writes an `InviteRevoke` entry (Owner).
    pub(crate) fn invite_revoke(
        &mut self,
        space_id: SpaceId,
        invite_id: InviteId,
    ) -> Result<MembersInfo, ApiError> {
        let row = self.shared_space_for_request(space_id)?;
        self.require_role(space_id, MemberRole::Owner)?;
        let now = self.clock.now_ms();
        let state = self
            .tentative_state(row.feed)?
            .ok_or_else(|| ApiError::not_found("the shared space is not known here"))?;
        let Some(record) = state.invite(invite_id) else {
            return Err(ApiError::invalid("inviteId", "this invite does not exist"));
        };
        if !matches!(record.status, InviteRecordStatus::Open) || record.is_expired(now) {
            return Err(ApiError::invalid("inviteId", "this invite is not open"));
        }
        self.membership_mint(
            row.feed,
            EntryBody::InviteRevoke { invite_id },
            Private::None,
            None,
        )?;
        self.push_event_once(Event::SpaceMembersChanged { space_id });
        self.push_event_once(Event::SyncRequested);
        self.members_info(space_id)
    }

    /// Starts the inviter side of the code flow (Owner, role is not Owner): the session waits for the
    /// task to open the mailbox. Emits `InviteChanged` and `InviteRequested`.
    pub(crate) fn invite_code_start(
        &mut self,
        space_id: SpaceId,
        role: MemberRole,
    ) -> Result<(), ApiError> {
        let row = self.shared_space_for_request(space_id)?;
        self.require_role(space_id, MemberRole::Owner)?;
        check_invite_role(role)?;
        let Some(server) = self.sync_server_url()? else {
            return Err(ApiError::invalid(
                "serverUrl",
                "turn sync on before inviting people",
            ));
        };
        // The invitee opens the invite on the server: the feed has to be there.
        let on_server = self
            .shared
            .timelines
            .get(&row.feed)
            .is_some_and(|t| !t.is_empty());
        if !on_server {
            return Err(ApiError::invalid(
                "spaceId",
                "the space is not on the server yet: try again in a moment",
            ));
        }
        self.check_invite_room(row.feed)?;
        self.invite_replace_session();
        let (channel, offer) = Session::opener(INVITE_TRANSCRIPT_DOMAIN, self.rng.as_ref());
        let mut session = InviteSession::new(InviteFlow::Code, InviteSide::Inviter, server);
        session.channel = Some(channel);
        session.outbox.push((SEQ_OFFER, offer));
        session.space_id = Some(space_id);
        session.feed = Some(row.feed);
        session.role = Some(role);
        session.space_name = Some(self.structure(space_id)?.model.space().name);
        self.invite = Some(session);
        self.invite_changed();
        self.events.push(Event::InviteRequested);
        Ok(())
    }

    /// The checks both invitee flows end with: a workspace and a kit, and a server this account can
    /// join a space on (its own, or none yet). `server` is validated already.
    fn invitee_preconditions(&mut self, server: &str) -> Result<(), ApiError> {
        if self
            .db
            .workspace_get()
            .map_err(map_storage_error)?
            .is_none()
        {
            return Err(ApiError::invalid(
                "workspace",
                "create a workspace before joining a shared space",
            ));
        }
        if self.sync_identity()?.is_none() {
            return Err(ApiError::invalid(
                "account",
                "create your recovery kit before joining a shared space",
            ));
        }
        if let Some(configured) = self.sync_server_url()?
            && !configured.eq_ignore_ascii_case(server)
        {
            return Err(ApiError::invalid(
                "serverUrl",
                "this account syncs with another server; a space on another server needs the whole account moved",
            ));
        }
        Ok(())
    }

    /// Starts the invitee side of the code flow: decodes the code against the server address
    /// (`InvalidInput` for `serverUrl`, `code`, `workspace`, `account`, in that order).
    pub(crate) fn invite_code_join(
        &mut self,
        server_url: &str,
        code: &str,
    ) -> Result<(), ApiError> {
        let server = validate_server_url(server_url)?;
        let parsed = PairingCode::decode(code.trim(), &server).map_err(|error| match error {
            kb_keys::KeysError::Corrupted(message) => ApiError::invalid("code", message),
            other => map_keys_error(other),
        })?;
        self.invitee_preconditions(&server)?;
        self.invite_replace_session();
        let mut session = InviteSession::new(InviteFlow::Code, InviteSide::Invitee, server);
        session.mailbox_id = Some(parsed.mailbox_id);
        self.invite = Some(session);
        self.invite_changed();
        self.events.push(Event::InviteRequested);
        Ok(())
    }

    /// Starts the invitee side of the link flow from `atomik-kb://invite?...` or the `/join` form
    /// (`InvalidInput` for `link`).
    pub(crate) fn invite_link_open(&mut self, link: &str) -> Result<(), ApiError> {
        let (server, secret) = parse_invite_link(link)?;
        self.invitee_preconditions(&server)?;
        self.invite_replace_session();
        let mut session = InviteSession::new(InviteFlow::Link, InviteSide::Invitee, server);
        session.secret = Some(secret);
        self.invite = Some(session);
        self.invite_changed();
        self.events.push(Event::InviteRequested);
        Ok(())
    }

    /// The digits match (code flow) or the preview is accepted (link flow).
    pub(crate) fn invite_confirm(&mut self) -> Result<(), ApiError> {
        let Some(session) = self.invite.as_ref() else {
            return Err(ApiError::invalid("invite", "there is nothing to confirm"));
        };
        let result = match (session.flow, session.side, session.state) {
            (InviteFlow::Link, InviteSide::Invitee, InviteSessionState::Previewing) => {
                self.invite_with(|core, session| core.invitee_accepts_preview(session))
            }
            (InviteFlow::Code, InviteSide::Invitee, InviteSessionState::Confirming) => {
                self.invite_with(|core, session| core.invitee_confirms(session))
            }
            (InviteFlow::Code, InviteSide::Inviter, InviteSessionState::PeerConfirming) => {
                self.invite_with(|core, session| core.inviter_confirms(session))
            }
            (InviteFlow::Code, InviteSide::Inviter, InviteSessionState::Confirming) => {
                return Err(ApiError::invalid(
                    "invite",
                    "the other person has not confirmed yet",
                ));
            }
            _ => {
                return Err(ApiError::invalid("invite", "there is nothing to confirm"));
            }
        };
        self.invite_changed();
        result
    }

    /// Ends the session.
    pub(crate) fn invite_cancel(&mut self) -> Result<(), ApiError> {
        // A session that ended stays as it ended (the dialog shows how); only a running one is ended.
        if self.invite.as_ref().is_some_and(InviteSession::is_active) {
            self.invite_with(|core, session| {
                core.invite_end(session, InviteSessionState::Cancelled, true);
                Ok(())
            })?;
            self.invite_changed();
        }
        Ok(())
    }

    /// A new session replaces the old one: an active one is ended first (rolled back, aborted).
    fn invite_replace_session(&mut self) {
        if self.invite.as_ref().is_some_and(InviteSession::is_active) {
            let _ = self.invite_with(|core, session| {
                core.invite_end(session, InviteSessionState::Cancelled, true);
                Ok(())
            });
        }
        self.invite = None;
    }

    /// Puts a session in a final state: what it did before the server took it is undone (a feed the
    /// invitee adopted, an `Invite` the inviter wrote and never handed out is revoked), and an
    /// abort goes to the other side when the channel is open.
    fn invite_end(&mut self, session: &mut InviteSession, state: InviteSessionState, abort: bool) {
        if abort
            && let Some(channel) = session.channel.as_ref()
            && channel.agreed()
        {
            let seq = session.last_seq + 2;
            let sealed = channel.seal(MessageKind::Abort, seq, &[], self.rng.as_ref());
            session.outbox.push((seq, sealed));
            session.last_seq = seq;
        }
        if session.side == InviteSide::Invitee {
            self.invitee_rollback(session);
        } else if session.invite_seq.is_some() && !session.bundle_released {
            // The `Invite` was written and no bundle ever left: nobody can use it.
            if let (Some(feed), Some(secret)) = (session.feed, session.secret.as_ref()) {
                let invite_id = derive_invite_id(secret);
                if let Err(error) = self.membership_mint(
                    feed,
                    EntryBody::InviteRevoke { invite_id },
                    Private::None,
                    None,
                ) {
                    tracing::debug!(code = ?error.code, "an unused invite could not be revoked");
                }
            }
        }
        session.state = state;
    }

    // ----- inviter, code flow ---------------------------------------------------------------

    /// The invitee confirmed first and sent its identity; the inviter shows it. Confirming mints the
    /// `Invite` (bound to that account, verified, an hour) and prepares the bundle, which leaves once
    /// the entry is on the server.
    fn inviter_confirms(&mut self, session: &mut InviteSession) -> Result<(), ApiError> {
        let (Some(space_id), Some(feed), Some(role), Some(peer)) = (
            session.space_id,
            session.feed,
            session.role,
            session.peer.clone(),
        ) else {
            return Err(ApiError::internal("invite session without its parts"));
        };
        let allowed = self.require_role(space_id, MemberRole::Owner).is_ok()
            && self.check_invite_room(feed).is_ok();
        if !allowed {
            self.invite_end(session, InviteSessionState::Failed, true);
            session.error = Some("you can no longer invite people to this space".to_owned());
            return Ok(());
        }
        let me = self.me()?;
        let now = self.clock.now_ms();
        let secret = InviteSecret::generate(self.rng.as_ref());
        let signing = invite_signing_key(&secret);
        let invite_id = derive_invite_id(&secret);
        let expires_at = now.saturating_add(
            i64::try_from(INVITE_CODE_TTL_SECONDS)
                .unwrap_or(3600)
                .saturating_mul(1000),
        );
        let entry = self.membership_mint(
            feed,
            EntryBody::Invite {
                invite_id,
                invite_pub: signing.verifying_key(),
                role,
                expires_at,
                bound_account: Some(peer.account_id),
                verified: true,
                enc_bundle: None,
            },
            Private::None,
            None,
        )?;
        let state = self
            .tentative_state(feed)?
            .ok_or_else(|| ApiError::not_found("the shared space is not known here"))?;
        let key = self.feed_sk(feed, state.epoch)?.ok_or_else(|| {
            ApiError::crypto("this device does not hold the space key of the current epoch")
        })?;
        session.bundle = Some(CodeBundle {
            feed_id: feed,
            space_id,
            inviter: me,
            secret: secret.clone(),
            epoch: state.epoch,
            space_key: key,
            membership_head: ChainHead {
                seq: entry.header.seq,
                hash: entry.hash(),
            },
            space_name: session.space_name.clone().unwrap_or_default(),
        });
        session.secret = Some(secret);
        session.invite_seq = Some(entry.header.seq);
        session.bundle_released = false;
        session.expires_at = Some(expires_at);
        session.verified = true;
        session.state = InviteSessionState::Transferring;
        // The digits authenticated both accounts (ADR-0042): the person is verified.
        self.ensure_account()?;
        if let Some(account) = self.docs.account.as_ref() {
            account
                .model
                .set_verified(peer.account_id, Some(now))
                .map_err(crate::documents::map_model_error)?;
        }
        self.persist_account()?;
        self.push_event_once(Event::PeopleChanged);
        self.push_event_once(Event::SpaceMembersChanged { space_id });
        self.push_event_once(Event::SyncRequested);
        self.invite_release_bundle_for(session);
        Ok(())
    }

    /// The inviter's bundle goes into the outbox once the `Invite` entry is on the server (the task
    /// posted it, or the sync driver did).
    fn invite_release_bundle_for(&mut self, session: &mut InviteSession) {
        if session.side != InviteSide::Inviter
            || session.state != InviteSessionState::Transferring
            || session.bundle_released
        {
            return;
        }
        let (Some(feed), Some(seq)) = (session.feed, session.invite_seq) else {
            return;
        };
        let on_server = self
            .fcfg_u64(feed, "posted")
            .ok()
            .flatten()
            .is_some_and(|posted| posted >= seq)
            || self
                .shared
                .timelines
                .get(&feed)
                .is_some_and(|t| u64::try_from(t.len()).unwrap_or(0) > seq);
        if !on_server {
            return;
        }
        let (Some(bundle), Some(channel)) = (session.bundle.as_ref(), session.channel.as_ref())
        else {
            return;
        };
        let sealed = channel.seal(
            MessageKind::InviteBundle,
            SEQ_BUNDLE,
            &bundle.encode(),
            self.rng.as_ref(),
        );
        session.outbox.push((SEQ_BUNDLE, sealed));
        session.last_seq = SEQ_BUNDLE;
        session.bundle_released = true;
    }

    /// Called when the server took membership entries of `feed` (the task's or the driver's post): an
    /// inviter whose `Invite` is among them releases its bundle.
    pub(crate) fn invite_entries_posted(&mut self, feed: FeedId) {
        let Some(mut session) = self.invite.take() else {
            return;
        };
        if session.feed == Some(feed) {
            self.invite_release_bundle_for(&mut session);
            if session.bundle_released {
                self.invite_changed();
            }
        }
        self.invite = Some(session);
    }

    /// Called when a membership entry folded: the invitee's `Join` of the invite this session wrote
    /// finishes the inviter's session.
    pub(crate) fn invite_join_seen(
        &mut self,
        feed: FeedId,
        entry: &kb_keys::membership::MembershipEntry,
    ) {
        let EntryBody::Join { invite_id, .. } = &entry.header.body else {
            return;
        };
        let Some(session) = self.invite.as_mut() else {
            return;
        };
        if session.side != InviteSide::Inviter
            || session.feed != Some(feed)
            || session.state != InviteSessionState::Transferring
        {
            return;
        }
        if session
            .secret
            .as_ref()
            .is_some_and(|secret| derive_invite_id(secret) == *invite_id)
        {
            session.state = InviteSessionState::Done;
            self.invite_changed();
        }
    }

    /// The inviter's `Invite` may have been dropped (the compare-and-swap was lost): the session notes
    /// it, and [`Self::invite_remint_pending`] writes it again once the log has caught up.
    pub(crate) fn invite_after_conflict(&mut self, feed: FeedId) {
        if let Some(session) = self.invite.as_mut()
            && session.side == InviteSide::Inviter
            && session.feed == Some(feed)
            && session.state == InviteSessionState::Transferring
            && !session.bundle_released
        {
            session.remint = true;
        }
    }

    /// Writes the inviter's `Invite` again with the same secret on the log as it is now, when the
    /// session noted that a lost compare-and-swap dropped it; the bundle then names the new head.
    pub(crate) fn invite_remint_pending(&mut self, feed: FeedId) {
        let Some(mut session) = self.invite.take() else {
            return;
        };
        if session.remint && session.feed == Some(feed) {
            session.remint = false;
            if let Err(error) = self.invite_remint(&mut session) {
                tracing::warn!(code = ?error.code, "the invite could not be written again");
                session.fail("the invite could not be written; try again");
                self.invite_changed();
            }
        }
        self.invite = Some(session);
    }

    fn invite_remint(&mut self, session: &mut InviteSession) -> Result<(), ApiError> {
        let (Some(feed), Some(secret), Some(role), Some(peer), Some(bundle)) = (
            session.feed,
            session.secret.clone(),
            session.role,
            session.peer.clone(),
            session.bundle.as_mut(),
        ) else {
            return Ok(());
        };
        let signing = invite_signing_key(&secret);
        let invite_id = derive_invite_id(&secret);
        // Still there (the entry made it into the log): nothing to do.
        if self
            .tentative_state(feed)?
            .is_some_and(|state| state.invite(invite_id).is_some())
        {
            return Ok(());
        }
        let expires_at = session.expires_at.unwrap_or_else(|| self.clock.now_ms());
        let entry = self.membership_mint(
            feed,
            EntryBody::Invite {
                invite_id,
                invite_pub: signing.verifying_key(),
                role,
                expires_at,
                bound_account: Some(peer.account_id),
                verified: true,
                enc_bundle: None,
            },
            Private::None,
            None,
        )?;
        bundle.membership_head = ChainHead {
            seq: entry.header.seq,
            hash: entry.hash(),
        };
        session.invite_seq = Some(entry.header.seq);
        Ok(())
    }

    // ----- invitee, code flow ---------------------------------------------------------------

    /// The invitee confirmed the digits: its identity goes to the inviter.
    fn invitee_confirms(&mut self, session: &mut InviteSession) -> Result<(), ApiError> {
        let Some(channel) = session.channel.as_ref() else {
            return Err(ApiError::internal("invite session without a channel"));
        };
        let log = self
            .security
            .log
            .as_ref()
            .ok_or_else(|| ApiError::invalid("account", "create your recovery kit first"))?;
        let Some(last) = log.entries().last() else {
            return Err(ApiError::invalid("account", "the device log is empty"));
        };
        let (entries, published) = self.sync_account_log()?;
        let on_server =
            self.invite_account_on_server(&session.server_url, entries.len(), published);
        let total: usize = entries.iter().map(Vec::len).sum();
        let identity = InviteIdentity {
            account_id: self.me()?,
            genesis_hash: genesis_hash(log),
            log_head: ChainHead {
                seq: last.seq,
                hash: last.hash(),
            },
            log: (!on_server && total <= kb_types::sharing::INVITE_BUNDLE_LOG_MAX_BYTES)
                .then_some(entries),
            display_name: self.invite_display_name()?,
        };
        let sealed = channel.seal(
            MessageKind::InviteIdentity,
            SEQ_IDENTITY,
            &identity.encode(),
            self.rng.as_ref(),
        );
        session.outbox.push((SEQ_IDENTITY, sealed));
        session.last_seq = SEQ_IDENTITY;
        session.state = InviteSessionState::PeerConfirming;
        Ok(())
    }

    /// The name this account gives a space it joins: its profile name, or a short stand-in made of
    /// its account id (changed later with `ProfileSet`).
    pub(crate) fn invite_display_name(&self) -> Result<String, ApiError> {
        Ok(match self.profile_name_of_account() {
            Some(name) => name,
            None => crate::membership::fallback_name(self.me()?),
        })
    }

    /// Whether this account's device log is on the server the session talks to.
    pub(crate) fn invite_account_on_server(
        &self,
        server_url: &str,
        entries: usize,
        published: u64,
    ) -> bool {
        let same = self
            .sync_server_url()
            .ok()
            .flatten()
            .is_some_and(|configured| configured.eq_ignore_ascii_case(server_url));
        same && published > 0 && usize::try_from(published).unwrap_or(usize::MAX) >= entries
    }

    // ----- host methods (`kb_net::InviteHost`, one to one) ---------------------------------

    /// The session as the dialogs show it (`state: Idle` when there is none).
    pub fn invite_info(&self) -> Result<InviteSessionInfo, ApiError> {
        let Some(session) = self.invite.as_ref() else {
            return Ok(InviteSessionInfo {
                flow: InviteFlow::Code,
                side: InviteSide::Inviter,
                state: InviteSessionState::Idle,
                code: None,
                sas: None,
                peer_name: None,
                peer_safety_number: None,
                space_name: None,
                inviter_name: None,
                role: None,
                verified: false,
                server_url: None,
                error: None,
                expires_at: None,
            });
        };
        let code = match (session.side, session.mailbox_id, session.flow) {
            (InviteSide::Inviter, Some(mailbox_id), InviteFlow::Code) => {
                Some(PairingCode { mailbox_id }.display(&session.server_url))
            }
            _ => None,
        };
        Ok(InviteSessionInfo {
            flow: session.flow,
            side: session.side,
            state: session.state,
            code,
            sas: session.channel.as_ref().and_then(Session::sas),
            peer_name: session.peer_name.clone(),
            peer_safety_number: session.peer_safety_number.clone(),
            space_name: session.space_name.clone(),
            inviter_name: session.inviter_name.clone(),
            role: session.role,
            verified: session.verified,
            server_url: Some(session.server_url.clone()),
            error: session.error.clone(),
            expires_at: session.expires_at,
        })
    }

    /// The mailbox of the session: the invitee's from the typed code, the inviter's once
    /// `invite_mailbox_opened` told the core.
    pub fn invite_mailbox_id(&self) -> Result<Option<[u8; 6]>, ApiError> {
        Ok(self.invite.as_ref().and_then(|s| s.mailbox_id))
    }

    /// The mailbox is open (inviter) or found (invitee), and when it expires (Unix ms).
    pub fn invite_mailbox_opened(
        &mut self,
        mailbox_id: [u8; 6],
        expires_at: i64,
    ) -> Result<(), ApiError> {
        let Some(session) = self.invite.as_mut() else {
            return Err(ApiError::not_found("no invite is running"));
        };
        session.mailbox_id = Some(mailbox_id);
        session.expires_at = Some(expires_at);
        if let Some(channel) = session.channel.as_mut() {
            channel.set_mailbox(mailbox_id);
        }
        if session.side == InviteSide::Inviter && session.state == InviteSessionState::Starting {
            session.state = InviteSessionState::Waiting;
        }
        self.invite_changed();
        Ok(())
    }

    /// Messages to post, by mailbox sequence (the inviter's offer is 0).
    pub fn invite_outbox(&self) -> Result<Vec<(u32, Vec<u8>)>, ApiError> {
        Ok(self
            .invite
            .as_ref()
            .map(|s| s.outbox.clone())
            .unwrap_or_default())
    }

    /// The message `seq` of the outbox was posted.
    pub fn invite_posted(&mut self, seq: u32) -> Result<(), ApiError> {
        let Some(session) = self.invite.as_mut() else {
            return Ok(());
        };
        session.outbox.retain(|(s, _)| *s != seq);
        Ok(())
    }

    /// A message of the other side (`from` is the sender's side), with its mailbox sequence.
    pub fn invite_receive(
        &mut self,
        seq: u32,
        from: InviteSide,
        bytes: &[u8],
    ) -> Result<(), ApiError> {
        let Some(session) = self.invite.as_ref() else {
            return Err(ApiError::not_found("no invite is running"));
        };
        if from == session.side || !session.is_active() || session.flow != InviteFlow::Code {
            return Ok(());
        }
        let outcome = self.invite_with(|core, session| core.invite_receive_in(session, seq, bytes));
        self.invite_changed();
        match outcome {
            Ok(()) => Ok(()),
            Err(error) => {
                if let Some(session) = self.invite.as_mut()
                    && session.is_active()
                {
                    session.fail(UNVERIFIED_MESSAGE);
                }
                Err(error)
            }
        }
    }

    fn invite_receive_in(
        &mut self,
        session: &mut InviteSession,
        seq: u32,
        bytes: &[u8],
    ) -> Result<(), ApiError> {
        match (session.side, seq) {
            (InviteSide::Inviter, SEQ_ANSWER) => {
                let Some(channel) = session.channel.as_mut() else {
                    return Err(ApiError::internal("inviter session without a channel"));
                };
                if channel.agreed() {
                    return Ok(());
                }
                let reveal = channel.accept_answer(bytes).map_err(map_keys_error)?;
                session.outbox.push((SEQ_REVEAL, reveal));
                session.last_seq = SEQ_REVEAL;
                session.state = InviteSessionState::Confirming;
            }
            (InviteSide::Inviter, SEQ_IDENTITY) => {
                let Some(channel) = session.channel.as_ref() else {
                    return Err(ApiError::internal("inviter session without a channel"));
                };
                match channel.open(bytes, SEQ_IDENTITY).map_err(map_keys_error)? {
                    (MessageKind::InviteIdentity, plain) => {
                        let identity = InviteIdentity::decode(&plain).map_err(map_keys_error)?;
                        self.inviter_takes_identity(session, identity)?;
                    }
                    (MessageKind::Abort, _) => session.state = InviteSessionState::Cancelled,
                    (other, _) => {
                        return Err(ApiError::crypto(format!(
                            "unexpected invite message {other:?}"
                        )));
                    }
                }
            }
            (InviteSide::Invitee, SEQ_OFFER) => {
                if session.channel.is_some() {
                    return Ok(());
                }
                let Some(mailbox_id) = session.mailbox_id else {
                    return Err(ApiError::internal("invitee session without a mailbox"));
                };
                let (channel, answer) = Session::typist(
                    INVITE_TRANSCRIPT_DOMAIN,
                    mailbox_id,
                    bytes,
                    self.rng.as_ref(),
                )
                .map_err(map_keys_error)?;
                session.channel = Some(channel);
                session.outbox.push((SEQ_ANSWER, answer));
                session.last_seq = SEQ_ANSWER;
            }
            (InviteSide::Invitee, SEQ_REVEAL) => {
                let Some(channel) = session.channel.as_mut() else {
                    return Err(ApiError::internal("invitee session without a channel"));
                };
                if channel.agreed() {
                    return Ok(());
                }
                channel.accept_reveal(bytes).map_err(map_keys_error)?;
                session.state = InviteSessionState::Confirming;
            }
            (InviteSide::Invitee, SEQ_BUNDLE) => {
                let Some(channel) = session.channel.as_ref() else {
                    return Err(ApiError::internal("invitee session without a channel"));
                };
                match channel.open(bytes, SEQ_BUNDLE).map_err(map_keys_error)? {
                    (MessageKind::InviteBundle, plain) => {
                        let bundle = CodeBundle::decode(&plain).map_err(map_keys_error)?;
                        self.invitee_takes_bundle(session, bundle)?;
                    }
                    (MessageKind::Abort, _) => session.state = InviteSessionState::Cancelled,
                    (other, _) => {
                        return Err(ApiError::crypto(format!(
                            "unexpected invite message {other:?}"
                        )));
                    }
                }
            }
            (_, other) => {
                // A later message of the other side after the known ones: an abort.
                if let Some(channel) = session.channel.as_ref()
                    && let Ok((MessageKind::Abort, _)) = channel.open(bytes, other)
                {
                    session.state = InviteSessionState::Cancelled;
                }
            }
        }
        Ok(())
    }

    /// The invitee's identity arrived: the inviter checks it hashes to the account id it names, shows
    /// who is asking and the safety number, and waits for its own confirmation.
    fn inviter_takes_identity(
        &mut self,
        session: &mut InviteSession,
        identity: InviteIdentity,
    ) -> Result<(), ApiError> {
        let me = self.me()?;
        if identity.account_id == me
            || identity.genesis_hash.get(..16) != Some(identity.account_id.as_bytes().as_slice())
        {
            return Err(ApiError::crypto(UNVERIFIED_MESSAGE));
        }
        if let Some(entries) = &identity.log {
            // The whole log came along (the account is not on the server): it must be the account's.
            let mut decoded = Vec::with_capacity(entries.len());
            for bytes in entries {
                decoded.push(
                    kb_keys::log::LogEntry::decode(bytes)
                        .map_err(|_| ApiError::crypto(UNVERIFIED_MESSAGE))?,
                );
            }
            let log = kb_keys::log::DeviceLog::from_entries(decoded)
                .map_err(|_| ApiError::crypto(UNVERIFIED_MESSAGE))?;
            let head = log.entries().last();
            let genuine = log.entries().first().map(kb_keys::log::account_id_of)
                == Some(identity.account_id)
                && genesis_hash(&log) == identity.genesis_hash
                && head.is_some_and(|last| {
                    last.seq == identity.log_head.seq && last.hash() == identity.log_head.hash
                });
            if !genuine {
                return Err(ApiError::crypto(UNVERIFIED_MESSAGE));
            }
            self.sync_foreign_log_received(identity.account_id, entries)?;
        }
        let own = self
            .security
            .log
            .as_ref()
            .map(genesis_hash)
            .ok_or_else(|| ApiError::invalid("account", "create your recovery kit first"))?;
        session.peer_name = Some(identity.display_name.clone());
        session.peer_safety_number = Some(safety_number(&own, &identity.genesis_hash));
        session.peer = Some(identity);
        session.state = InviteSessionState::PeerConfirming;
        Ok(())
    }

    /// The task gave up; `message` is shown to the user.
    pub fn invite_failed(&mut self, message: &str) -> Result<(), ApiError> {
        let Some(mut session) = self.invite.take() else {
            return Ok(());
        };
        if session.is_active() {
            let message: String = message.chars().take(200).collect();
            self.invite_end(&mut session, InviteSessionState::Failed, false);
            session.error = Some(message);
            self.invite_changed();
        }
        self.invite = Some(session);
        Ok(())
    }

    /// The inviter's `Invite` entry (and any other membership entry the session minted) that must be
    /// on the server before the bundle leaves: the feed and the encoded entries, in order.
    pub fn invite_membership_pending(&self) -> Result<Option<PendingMembership>, ApiError> {
        let Some(session) = self.invite.as_ref() else {
            return Ok(None);
        };
        if session.side != InviteSide::Inviter
            || session.state != InviteSessionState::Transferring
            || session.bundle_released
        {
            return Ok(None);
        }
        let Some(feed) = session.feed else {
            return Ok(None);
        };
        let posted = self.fcfg_u64(feed, "posted")?;
        let rows = self
            .db
            .membership_entries_list(feed, None)
            .map_err(map_storage_error)?;
        let entries: Vec<Vec<u8>> = rows
            .into_iter()
            .filter(|row| row.cursor == 0 && posted.is_none_or(|posted| row.seq > posted))
            .map(|row| row.bytes)
            .collect();
        if entries.is_empty() {
            return Ok(None);
        }
        Ok(Some(PendingMembership { feed, entries }))
    }

    /// The server took those entries; its membership head after them.
    pub fn invite_membership_posted(&mut self, head: MembershipHead) -> Result<(), ApiError> {
        let Some(feed) = self.invite.as_ref().and_then(|s| s.feed) else {
            return Ok(());
        };
        self.sync_membership_posted(feed, head)?;
        Ok(())
    }

    /// The invitee has no account on the server: its device log and the share signup claim
    /// (`kb_keys::invite::sign_signup` over the server epoch); `None` when the account exists there.
    pub fn invite_signup(&self, server_epoch: [u8; 16]) -> Result<Option<InviteSignup>, ApiError> {
        let Some(session) = self.invite.as_ref() else {
            return Ok(None);
        };
        if session.side != InviteSide::Invitee {
            return Ok(None);
        }
        let Some(secret) = session.secret.as_ref() else {
            return Ok(None);
        };
        let (entries, published) = self.sync_account_log()?;
        if self.invite_account_on_server(&session.server_url, entries.len(), published) {
            return Ok(None);
        }
        let me = self.me()?;
        Ok(Some(InviteSignup {
            invite_id: derive_invite_id(secret),
            log: entries,
            signature: sign_signup(&invite_signing_key(secret), me, server_epoch),
        }))
    }

    /// The signed request to open the invite (`kb_keys::invite::sign_open` over the server epoch).
    pub fn invite_open_request(
        &self,
        server_epoch: [u8; 16],
    ) -> Result<Option<InviteOpenRequest>, ApiError> {
        let Some(session) = self.invite.as_ref() else {
            return Ok(None);
        };
        if session.side != InviteSide::Invitee {
            return Ok(None);
        }
        let Some(secret) = session.secret.as_ref() else {
            return Ok(None);
        };
        let invite_id = derive_invite_id(secret);
        Ok(Some(InviteOpenRequest {
            invite_id,
            signature: sign_open(&invite_signing_key(secret), invite_id, server_epoch),
        }))
    }

    /// The server's answer to the open request: the feed and its membership log. The core verifies
    /// it (an Owner wrote the `Invite`, the head matches or extends the bundle's, the key matches the
    /// commitment) and moves to the preview (link flow) or to joining (code flow). While a signer's
    /// device log is missing it asks for it (`sync_needed_logs`) and leaves the session as it is:
    /// the host fetches the logs, hands them to `sync_foreign_log_received` and calls this again
    /// (the loop: `invite_opened`, then `sync_needed_logs`; empty means verification is over). A log
    /// that does not verify ends the session as `Failed`: the host reads `invite_info().state`, this
    /// still answers `Ok`.
    pub fn invite_opened(&mut self, opened: &InviteOpened) -> Result<(), ApiError> {
        let result = self.invite_with(|core, session| core.invitee_opened(session, opened));
        self.invite_changed();
        result
    }

    /// The `Join` entry to post once the user accepted: the feed and the encoded entry.
    pub fn invite_join(&mut self) -> Result<Option<(FeedId, Vec<u8>)>, ApiError> {
        if self.invite.is_none() {
            return Ok(None);
        }
        self.invite_with(|core, session| core.invitee_join(session))
    }

    /// The server took the `Join`; the core stores the key, writes `sharedSpaces`, creates the local
    /// space and bootstraps the feed.
    pub fn invite_joined(&mut self, head: MembershipHead) -> Result<(), ApiError> {
        if self.invite.is_none() {
            return Err(ApiError::not_found("no invite is running"));
        }
        let result = self.invite_with(|core, session| core.invitee_joined(session, head));
        self.invite_changed();
        result
    }
}

/// An invite grants Editor, Commenter or Viewer, never Owner.
fn check_invite_role(role: MemberRole) -> Result<(), ApiError> {
    if role.is_owner() {
        return Err(ApiError::invalid(
            "role",
            "an invite cannot grant the Owner role: promote a member instead",
        ));
    }
    Ok(())
}
