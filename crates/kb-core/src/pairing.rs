//! Device pairing (Milestone 7, ADR-0022): the sans-IO session on both sides. The
//! shell's pairing task (kb-net) opens the mailbox, posts what [`Core::pairing_outbox`]
//! hands it and feeds back what it fetched; the user confirms the digits through
//! `PairingConfirm`. Message sequences in the mailbox: 0 the joiner's offer (a
//! commitment to its ephemeral key), 1 the sponsor's answer (its ephemeral key), 2
//! the joiner's reveal (all three in the clear), 3 the joiner's sealed identity, 4
//! the sponsor's sealed bundle; an abort takes the next sequence.

use kb_keys::pairing::{Bundle, Identity, MessageKind, PairingCode, Session};
use kb_keys::recovery::RecoveryBlob;
use kb_keys::{KeysError, LogSigner};
use kb_storage::{DeviceLogRow, SecurityQueries};
use kb_types::devices::MAILBOX_ID_LEN;
use kb_types::{ApiError, Event, PairingInfo, PairingRole, PairingState, Request, Response};

use crate::recovery::{JoinedAccount, validate_server_url};
use crate::{Core, map_keys_error, map_storage_error};

const SEQ_OFFER: u32 = 0;
const SEQ_ANSWER: u32 = 1;
const SEQ_REVEAL: u32 = 2;
const SEQ_IDENTITY: u32 = 3;
const SEQ_BUNDLE: u32 = 4;

/// The pairing session this core runs, if any.
pub(crate) struct PairingSession {
    role: PairingRole,
    state: PairingState,
    server_url: String,
    session: Option<Session>,
    mailbox_id: Option<[u8; MAILBOX_ID_LEN]>,
    expires_at: Option<i64>,
    peer_name: Option<String>,
    peer_identity: Option<Identity>,
    error: Option<String>,
    /// Messages to post, by sequence.
    outbox: Vec<(u32, Vec<u8>)>,
    /// The sequence of this side's last message.
    last_seq: u32,
}

impl PairingSession {
    fn fail(&mut self, message: &str) {
        self.state = PairingState::Failed;
        self.error = Some(message.to_owned());
    }
}

impl Core {
    /// Routes the pairing requests.
    pub(crate) fn dispatch_pairing(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::PairingStart {
                role,
                server_url,
                code,
            } => {
                self.pairing_start(role, server_url.as_deref(), code.as_deref())?;
                Ok(Response::Pairing(self.pairing_info()))
            }
            Request::PairingConfirm => {
                self.pairing_confirm()?;
                Ok(Response::Pairing(self.pairing_info()))
            }
            Request::PairingCancel => {
                self.pairing_cancel()?;
                Ok(Response::Pairing(self.pairing_info()))
            }
            Request::PairingStatus => Ok(Response::Pairing(self.pairing_info())),
            other => Err(ApiError::internal(format!(
                "{} is not a pairing request",
                other.kind()
            ))),
        }
    }

    /// The session as the UI shows it.
    pub fn pairing_info(&self) -> PairingInfo {
        match &self.pairing {
            Some(session) => PairingInfo {
                role: session.role,
                state: session.state,
                code: match (session.role, session.mailbox_id) {
                    (PairingRole::Joiner, Some(mailbox_id)) => {
                        Some(PairingCode { mailbox_id }.display(&session.server_url))
                    }
                    _ => None,
                },
                sas: session.session.as_ref().and_then(Session::sas),
                peer_name: session.peer_name.clone(),
                server_url: Some(session.server_url.clone()),
                error: session.error.clone(),
                expires_at: session.expires_at,
            },
            None => PairingInfo {
                role: PairingRole::Joiner,
                state: PairingState::Idle,
                code: None,
                sas: None,
                peer_name: None,
                server_url: None,
                error: None,
                expires_at: None,
            },
        }
    }

    fn pairing_start(
        &mut self,
        role: PairingRole,
        server_url: Option<&str>,
        code: Option<&str>,
    ) -> Result<(), ApiError> {
        if self.pairing.as_ref().is_some_and(|s| {
            matches!(
                s.state,
                PairingState::Starting
                    | PairingState::Waiting
                    | PairingState::Confirming
                    | PairingState::PeerConfirming
                    | PairingState::Transferring
            )
        }) {
            return Err(ApiError::already_exists("a pairing is already running"));
        }
        let has_workspace = self
            .db
            .workspace_get()
            .map_err(map_storage_error)?
            .is_some();
        let session = match role {
            PairingRole::Joiner => {
                if has_workspace {
                    return Err(ApiError::already_exists(
                        "this installation already has a workspace; pairing needs a fresh one",
                    ));
                }
                let server_url = validate_server_url(server_url.unwrap_or(""))?;
                let (session, offer) = Session::joiner(self.rng.as_ref());
                PairingSession {
                    role,
                    state: PairingState::Starting,
                    server_url,
                    session: Some(session),
                    mailbox_id: None,
                    expires_at: None,
                    peer_name: None,
                    peer_identity: None,
                    error: None,
                    outbox: vec![(SEQ_OFFER, offer)],
                    last_seq: SEQ_OFFER,
                }
            }
            PairingRole::Sponsor => {
                if !has_workspace || self.sync_identity()?.is_none() {
                    return Err(ApiError::invalid(
                        "code",
                        "create your recovery kit and connect to a server before adding a device",
                    ));
                }
                let Some(server_url) = self.sync_server_url()? else {
                    return Err(ApiError::invalid(
                        "code",
                        "connect this device to a sync server first",
                    ));
                };
                let code = code.unwrap_or("").trim();
                let parsed = PairingCode::decode(code, &server_url).map_err(|_| {
                    ApiError::invalid(
                        "code",
                        "this code is not for this server, or it was mistyped",
                    )
                })?;
                PairingSession {
                    role,
                    state: PairingState::Starting,
                    server_url,
                    session: None,
                    mailbox_id: Some(parsed.mailbox_id),
                    expires_at: None,
                    peer_name: None,
                    peer_identity: None,
                    error: None,
                    outbox: Vec::new(),
                    last_seq: SEQ_OFFER,
                }
            }
        };
        self.pairing = Some(session);
        self.events.push(Event::PairingChanged);
        self.events.push(Event::PairingRequested);
        Ok(())
    }

    /// The user compared the digits: the joiner sends its identity, the sponsor adds
    /// the device to the log and sends the bundle.
    fn pairing_confirm(&mut self) -> Result<(), ApiError> {
        let now = self.clock.now_ms();
        let Some(session) = self.pairing.as_mut() else {
            return Err(ApiError::not_found("no pairing is running"));
        };
        if session.state != PairingState::Confirming {
            return Err(ApiError::invalid(
                "state",
                "there is nothing to confirm right now",
            ));
        }
        let Some(agreement) = session.session.as_ref() else {
            return Err(ApiError::internal(
                "pairing session without a key agreement",
            ));
        };
        match session.role {
            PairingRole::Joiner => {
                let identity = Identity {
                    device_id: self.device.device_id,
                    keys: self.security.device_keys.public(),
                    name: self.device.name.clone(),
                };
                let sealed = agreement.seal(
                    MessageKind::Identity,
                    SEQ_IDENTITY,
                    &identity.encode(),
                    self.rng.as_ref(),
                );
                session.outbox.push((SEQ_IDENTITY, sealed));
                session.last_seq = SEQ_IDENTITY;
                session.state = PairingState::PeerConfirming;
                self.events.push(Event::PairingChanged);
                Ok(())
            }
            PairingRole::Sponsor => {
                let Some(identity) = session.peer_identity.clone() else {
                    return Err(ApiError::invalid(
                        "state",
                        "the other device has not confirmed yet",
                    ));
                };
                let server_url = session.server_url.clone();
                // The device joins the log (signed here) before the bundle goes.
                let name = kb_keys::log::encrypt_name(
                    &self.ark,
                    identity.device_id,
                    &identity.name,
                    self.rng.as_ref(),
                );
                let me = self.device.device_id;
                let log = self
                    .security
                    .log
                    .as_mut()
                    .ok_or_else(|| ApiError::internal("no device log"))?;
                let entry = log
                    .add_device(
                        identity.device_id,
                        identity.keys,
                        name,
                        LogSigner::Device(&self.security.device_keys, me),
                        now,
                    )
                    .map_err(|e| map_keys_error(KeysError::Log(e)))?
                    .clone();
                let state = log
                    .verify()
                    .map_err(|e| map_keys_error(KeysError::Log(e)))?;
                let entries: Vec<Vec<u8>> = log.entries().iter().map(|e| e.encode()).collect();
                self.db
                    .device_log_append(&DeviceLogRow {
                        seq: entry.seq,
                        entry: entry.encode(),
                        hash: entry.hash(),
                    })
                    .map_err(map_storage_error)?;
                let account_id = state.account_id;
                self.security.log_state = Some(state);
                let recovery =
                    self.security.recovery.as_ref().ok_or_else(|| {
                        ApiError::invalid("state", "create your recovery kit first")
                    })?;
                let workspace = self
                    .db
                    .workspace_get()
                    .map_err(map_storage_error)?
                    .ok_or_else(|| ApiError::internal("workspace row missing"))?;
                let bundle = Bundle {
                    ark: self.ark.clone(),
                    ark_generation: self.security.ark_generation,
                    account_id,
                    server_url,
                    log: entries,
                    recovery_id: recovery.id,
                    recovery_blob: RecoveryBlob {
                        account_id,
                        generation: self.security.ark_generation,
                        wrapped: recovery.blob.clone(),
                    }
                    .encode(),
                    workspace_id: workspace.id,
                    workspace_name: workspace.name,
                };
                let Some(session) = self.pairing.as_mut() else {
                    return Err(ApiError::internal("pairing session vanished"));
                };
                let Some(agreement) = session.session.as_ref() else {
                    return Err(ApiError::internal(
                        "pairing session without a key agreement",
                    ));
                };
                let sealed = agreement.seal(
                    MessageKind::Bundle,
                    SEQ_BUNDLE,
                    &bundle.encode(),
                    self.rng.as_ref(),
                );
                session.outbox.push((SEQ_BUNDLE, sealed));
                session.last_seq = SEQ_BUNDLE;
                session.state = PairingState::Transferring;
                self.events.push(Event::PairingChanged);
                self.events.push(Event::SecurityChanged);
                self.events.push(Event::DevicesChanged {
                    added: self
                        .device_list()?
                        .into_iter()
                        .filter(|d| d.device_id == identity.device_id)
                        .collect(),
                    revoked: Vec::new(),
                    this_device_revoked: false,
                    rotated: false,
                });
                self.sync_note_local_change();
                Ok(())
            }
        }
    }

    fn pairing_cancel(&mut self) -> Result<(), ApiError> {
        let Some(session) = self.pairing.as_mut() else {
            return Ok(());
        };
        if matches!(
            session.state,
            PairingState::Done | PairingState::Failed | PairingState::Cancelled
        ) {
            self.pairing = None;
        } else {
            if let Some(agreement) = session.session.as_ref()
                && agreement.agreed()
            {
                let seq = session.last_seq + 2;
                let sealed = agreement.seal(MessageKind::Abort, seq, &[], self.rng.as_ref());
                session.outbox.push((seq, sealed));
                session.last_seq = seq;
            }
            session.state = PairingState::Cancelled;
        }
        self.events.push(Event::PairingChanged);
        Ok(())
    }

    // ----- for the shell's pairing task (kb-net) ------------------------------------------

    /// The mailbox of the session (the sponsor knows it from the code; the joiner
    /// once the server answered the offer).
    pub fn pairing_mailbox_id(&self) -> Result<Option<[u8; MAILBOX_ID_LEN]>, ApiError> {
        Ok(self.pairing.as_ref().and_then(|s| s.mailbox_id))
    }

    /// The mailbox is open (joiner) or found (sponsor).
    pub fn pairing_mailbox_opened(
        &mut self,
        mailbox_id: [u8; MAILBOX_ID_LEN],
        expires_at: i64,
    ) -> Result<(), ApiError> {
        let Some(session) = self.pairing.as_mut() else {
            return Err(ApiError::not_found("no pairing is running"));
        };
        session.mailbox_id = Some(mailbox_id);
        session.expires_at = Some(expires_at);
        if let Some(agreement) = session.session.as_mut() {
            agreement.set_mailbox(mailbox_id);
        }
        if session.role == PairingRole::Joiner && session.state == PairingState::Starting {
            session.state = PairingState::Waiting;
        }
        self.events.push(Event::PairingChanged);
        Ok(())
    }

    /// Messages to post, by sequence number, with their bytes.
    pub fn pairing_outbox(&mut self) -> Result<Vec<(u32, Vec<u8>)>, ApiError> {
        Ok(self
            .pairing
            .as_ref()
            .map(|s| s.outbox.clone())
            .unwrap_or_default())
    }

    /// A message was posted: it leaves the outbox; the sponsor's bundle completes the
    /// session.
    pub fn pairing_posted(&mut self, seq: u32) -> Result<(), ApiError> {
        let Some(session) = self.pairing.as_mut() else {
            return Ok(());
        };
        session.outbox.retain(|(s, _)| *s != seq);
        if session.role == PairingRole::Sponsor
            && seq == SEQ_BUNDLE
            && session.state == PairingState::Transferring
        {
            session.state = PairingState::Done;
            self.events.push(Event::PairingChanged);
        }
        Ok(())
    }

    /// A message from the other side arrived (`role` is the sender's).
    pub fn pairing_receive(
        &mut self,
        seq: u32,
        role: PairingRole,
        bytes: &[u8],
    ) -> Result<(), ApiError> {
        let Some(session) = self.pairing.as_mut() else {
            return Err(ApiError::not_found("no pairing is running"));
        };
        if role == session.role {
            return Ok(());
        }
        if matches!(
            session.state,
            PairingState::Done | PairingState::Failed | PairingState::Cancelled
        ) {
            return Ok(());
        }
        let outcome = match (session.role, seq) {
            (PairingRole::Sponsor, SEQ_OFFER) => {
                let Some(mailbox_id) = session.mailbox_id else {
                    return Err(ApiError::internal("sponsor session without a mailbox"));
                };
                match Session::sponsor(mailbox_id, bytes, self.rng.as_ref()) {
                    Ok((agreement, answer)) => {
                        session.session = Some(agreement);
                        session.outbox.push((SEQ_ANSWER, answer));
                        session.last_seq = SEQ_ANSWER;
                        session.state = PairingState::Waiting;
                        Ok(())
                    }
                    Err(err) => Err(err),
                }
            }
            (PairingRole::Sponsor, SEQ_REVEAL) => match session.session.as_mut() {
                Some(agreement) => match agreement.accept_reveal(bytes) {
                    Ok(()) => {
                        session.state = PairingState::Confirming;
                        Ok(())
                    }
                    Err(err) => Err(err),
                },
                None => Err(KeysError::Corrupted("no sponsor session".to_owned())),
            },
            (PairingRole::Joiner, SEQ_ANSWER) => match session.session.as_mut() {
                Some(agreement) => match agreement.accept_answer(bytes) {
                    Ok(reveal) => {
                        session.outbox.push((SEQ_REVEAL, reveal));
                        session.last_seq = SEQ_REVEAL;
                        session.state = PairingState::Confirming;
                        Ok(())
                    }
                    Err(err) => Err(err),
                },
                None => Err(KeysError::Corrupted("no joiner session".to_owned())),
            },
            (PairingRole::Sponsor, SEQ_IDENTITY) => match session.session.as_ref() {
                Some(agreement) => match agreement.open(bytes, SEQ_IDENTITY) {
                    Ok((MessageKind::Identity, plain)) => match Identity::decode(&plain) {
                        Ok(identity) => {
                            session.peer_name = Some(identity.name.clone());
                            session.peer_identity = Some(identity);
                            Ok(())
                        }
                        Err(err) => Err(err),
                    },
                    Ok((MessageKind::Abort, _)) => {
                        session.state = PairingState::Cancelled;
                        Ok(())
                    }
                    Ok((other, _)) => Err(KeysError::Corrupted(format!(
                        "unexpected pairing message {other:?}"
                    ))),
                    Err(err) => Err(err),
                },
                None => Err(KeysError::Corrupted("no sponsor session".to_owned())),
            },
            (PairingRole::Joiner, SEQ_BUNDLE) => match session.session.as_ref() {
                Some(agreement) => match agreement.open(bytes, SEQ_BUNDLE) {
                    Ok((MessageKind::Bundle, plain)) => match Bundle::decode(&plain) {
                        Ok(bundle) => {
                            session.state = PairingState::Transferring;
                            session.peer_name = None;
                            let joined = JoinedAccount {
                                recovery_blob: match RecoveryBlob::decode(&bundle.recovery_blob) {
                                    Ok(blob) => blob.wrapped,
                                    Err(err) => {
                                        session.fail("the bundle is not readable");
                                        self.events.push(Event::PairingChanged);
                                        return Err(map_keys_error(err));
                                    }
                                },
                                ark: bundle.ark,
                                ark_generation: bundle.ark_generation,
                                account_id: bundle.account_id,
                                log: match bundle
                                    .log
                                    .iter()
                                    .map(|b| kb_keys::LogEntry::decode(b))
                                    .collect::<Result<Vec<_>, _>>()
                                {
                                    Ok(log) => log,
                                    Err(err) => {
                                        session.fail("the bundle's device log is not readable");
                                        self.events.push(Event::PairingChanged);
                                        return Err(map_keys_error(KeysError::Log(err)));
                                    }
                                },
                                published_up_to: None,
                                recovery_id: bundle.recovery_id,
                                recovery_created_at: self.clock.now_ms(),
                                workspace: kb_storage::WorkspaceRow {
                                    id: bundle.workspace_id,
                                    name: bundle.workspace_name,
                                    created_at: self.clock.now_ms(),
                                },
                                server_url: bundle.server_url,
                                kit_stale: false,
                            };
                            // The log the sponsor sent is published (it sent it first);
                            // every entry counts as such.
                            let joined = JoinedAccount {
                                published_up_to: joined.log.last().map(|e| e.seq),
                                ..joined
                            };
                            match self.install_joined_account(joined) {
                                Ok(()) => {
                                    if let Some(session) = self.pairing.as_mut() {
                                        session.state = PairingState::Done;
                                    }
                                    self.events.push(Event::PairingChanged);
                                    return Ok(());
                                }
                                Err(err) => {
                                    if let Some(session) = self.pairing.as_mut() {
                                        session.fail(&err.message);
                                    }
                                    self.events.push(Event::PairingChanged);
                                    return Err(err);
                                }
                            }
                        }
                        Err(err) => Err(err),
                    },
                    Ok((MessageKind::Abort, _)) => {
                        session.state = PairingState::Cancelled;
                        Ok(())
                    }
                    Ok((other, _)) => Err(KeysError::Corrupted(format!(
                        "unexpected pairing message {other:?}"
                    ))),
                    Err(err) => Err(err),
                },
                None => Err(KeysError::Corrupted("no joiner session".to_owned())),
            },
            (_, other_seq) => {
                // A later message from the peer after the known ones: an abort.
                if let Some(agreement) = session.session.as_ref()
                    && let Ok((MessageKind::Abort, _)) = agreement.open(bytes, other_seq)
                {
                    session.state = PairingState::Cancelled;
                }
                Ok(())
            }
        };
        match outcome {
            Ok(()) => {
                self.events.push(Event::PairingChanged);
                Ok(())
            }
            Err(err) => {
                if let Some(session) = self.pairing.as_mut() {
                    session.fail("the other device could not be verified");
                }
                self.events.push(Event::PairingChanged);
                Err(map_keys_error(err))
            }
        }
    }

    /// The task gave up (network, expiry).
    pub fn pairing_failed(&mut self, message: &str) -> Result<(), ApiError> {
        if let Some(session) = self.pairing.as_mut()
            && !matches!(session.state, PairingState::Done | PairingState::Cancelled)
        {
            session.fail(message);
            self.events.push(Event::PairingChanged);
        }
        Ok(())
    }
}
