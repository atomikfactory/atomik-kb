//! Device pairing (ADR-0022, `docs/development/milestone-07.md`, "kb-keys"): the pairing
//! code, the ephemeral key agreement with a short authentication string, the sealed
//! channel messages and the bundle the sponsor hands over. Pure and wasm32-capable;
//! the mailbox transport lives in `kb-net`.
//!
//! # Flow
//!
//! The joiner commits to its ephemeral key before it sees the sponsor's, and the
//! sponsor answers with its key before it sees the joiner's: nobody, the relay
//! included, can pick a key after seeing the other side's, so a relay that wants
//! both screens to show the same six digits has one chance in a million per attempt
//! instead of a search it can run offline. Mailbox sequence numbers:
//!
//! ```text
//! 0 offer     joiner -> sponsor   clear   commitment to e_joiner
//! 1 answer    sponsor -> joiner   clear   e_sponsor
//! 2 reveal    joiner -> sponsor   clear   e_joiner (the sponsor checks the commitment)
//!   -- both sides show the digits; the joiner confirms first --
//! 3 identity  joiner -> sponsor   sealed  (MessageKind::Identity, seq 3)
//!   -- the sponsor shows the name and confirms --
//! 4 bundle    sponsor -> joiner   sealed  (MessageKind::Bundle, seq 4)
//!   an abort is sealed with the sender's next sequence number
//! ```
//!
//! # Wire formats (pairing version 1)
//!
//! ```text
//! code        12 Crockford base32 symbols (0-9 A-Z without I L O U), most significant
//!             first, of the 60-bit value  mailbox_id (48 bits) || checksum (12 bits);
//!             checksum = the first 12 bits, big-endian, of
//!             BLAKE3(PAIRING_CODE_DOMAIN || canonical server_url || mailbox_id),
//!             canonical = surrounding whitespace and trailing '/' removed, ASCII
//!             letters lowercased
//! offer       u8 version = 1 | u8 role = 0 (joiner)  | [32] BLAKE3(PAIRING_COMMIT_DOMAIN || e_joiner)
//! answer      u8 version = 1 | u8 role = 1 (sponsor) | [32] e_sponsor
//! reveal      u8 version = 1 | u8 role = 0 (joiner)  | [32] e_joiner
//! transcript  T = PAIRING_TRANSCRIPT_DOMAIN || [6] mailbox_id || [32] e_joiner || [32] e_sponsor
//! secrets     ss = X25519(e_mine, e_theirs)
//!             channel key = HKDF(ss, pairing-channel, info = T)
//!             SAS = (u32 big-endian of HKDF(ss, pairing-sas, info = T)[..4]) mod 10^6,
//!                   six decimal digits, zero-padded
//! message     Pairing envelope (ADR-0013), unsigned, under the channel key, header
//!             u8 role (the sender's) | u32 LE seq | u8 kind (1 identity, 2 bundle, 3 abort)
//! identity    [16] device_id | [32] signing key | [32] agreement key |
//!             u32 LE name_len | [64] name: UTF-8, then zero bytes      (148 bytes)
//! bundle      [32] ark | u32 ark_generation | [16] account_id | u32 len + server_url |
//!             u32 count | count x (u32 len + encoded log entry) | [32] recovery_id |
//!             u32 len + recovery_blob | [16] workspace_id | u32 len + workspace_name
//! ```
//!
//! Integers are little-endian unless stated otherwise; decoding is strict (canonical
//! encodings only, no trailing bytes). Errors never carry key material, the SAS or
//! the typed text; a code's mailbox id is not secret.

use kb_crypto::agreement::AGREEMENT_PUBLIC_LEN;
use kb_crypto::{
    AgreementPublic, AgreementSecret, CryptoError, EnvelopeKind, Label, Secret32, VerifyingKey,
};
use kb_types::devices::{
    DEVICE_NAME_PAD, MAILBOX_ID_LEN, PAIRING_CODE_DOMAIN, PAIRING_CODE_LEN,
    PAIRING_TRANSCRIPT_DOMAIN, SAS_DIGITS,
};
use kb_types::{AccountId, DeviceId, Rng, WorkspaceId};
use zeroize::Zeroizing;

use crate::codec::{DecodeError, Reader, put_bytes};
use crate::device::DevicePublicKeys;
use crate::{Ark, KeysError};

/// Version byte of the offer, the answer and the reveal.
const PAIRING_VERSION: u8 = 1;
/// Domain of the joiner's commitment to its ephemeral key (in the offer).
const PAIRING_COMMIT_DOMAIN: &str = "atomik-kb/v1/pairing-commit";
/// Bytes of an offer, an answer or a reveal.
const HELLO_LEN: usize = 2 + AGREEMENT_PUBLIC_LEN;
/// Bytes of a sealed message's header.
const HEADER_LEN: usize = 6;
/// Bits of checksum at the end of a pairing code.
const CHECKSUM_BITS: u32 = 12;
/// Bits per code symbol.
const SYMBOL_BITS: usize = 5;
/// The Crockford base32 alphabet.
pub(crate) const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// `10^SAS_DIGITS`.
const SAS_MODULUS: u32 = 1_000_000;
/// Bytes of an encoded [`Identity`].
const IDENTITY_LEN: usize = 16 + 32 + 32 + 4 + DEVICE_NAME_PAD;

// The constants of `kb_types::devices` and the layout above must agree.
const _: () =
    assert!(PAIRING_CODE_LEN * SYMBOL_BITS == MAILBOX_ID_LEN * 8 + CHECKSUM_BITS as usize);
const _: () = assert!(MAILBOX_ID_LEN * 8 + (CHECKSUM_BITS as usize) <= 64);
const _: () = assert!(SAS_DIGITS == 6 && SAS_MODULUS == 1_000_000);

/// Which side of the session this is. A session is run by an **opener**, who creates the mailbox,
/// makes the offer (the commitment to its ephemeral key) and reveals it, and a **typist**, who types
/// the code, answers with its own key and takes the reveal. Device pairing (ADR-0022) is the opener
/// = the fresh installation that joins ([`Session::joiner`]) and the typist = the existing device
/// that admits it ([`Session::sponsor`]); an invite to a shared space (ADR-0046) is the opener =
/// the inviting Owner and the typist = the invitee ([`Session::opener`], [`Session::typist`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Creates the mailbox, makes the offer, reveals (byte `0`).
    Opener,
    /// Types the code, answers, takes the reveal (byte `1`).
    Typist,
}

impl Role {
    /// The wire byte (`0` joiner, `1` sponsor).
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        match self {
            Role::Opener => 0,
            Role::Typist => 1,
        }
    }

    /// Parses the wire byte.
    pub fn from_byte(byte: u8) -> Result<Self, KeysError> {
        match byte {
            0 => Ok(Role::Opener),
            1 => Ok(Role::Typist),
            other => Err(KeysError::Corrupted(format!(
                "unknown pairing role {other}"
            ))),
        }
    }

    /// The other side.
    #[must_use]
    pub const fn peer(self) -> Self {
        match self {
            Role::Opener => Role::Typist,
            Role::Typist => Role::Opener,
        }
    }
}

/// What a sealed message carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// The joiner's device id, public keys and name (after the joiner confirmed).
    Identity,
    /// The sponsor's bundle (after the sponsor confirmed).
    Bundle,
    /// Either side gave up; the plaintext is an empty byte string.
    Abort,
    /// An invite session (ADR-0046): the invitee's account, genesis hash, log head, log (when it is
    /// not on the server yet) and display name ([`InviteIdentity`]), sent after the invitee
    /// confirmed the digits.
    InviteIdentity,
    /// An invite session: the inviter's bundle (`invite::CodeBundle`), sent after the inviter
    /// confirmed.
    InviteBundle,
}

impl MessageKind {
    /// The wire byte.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        match self {
            MessageKind::Identity => 1,
            MessageKind::Bundle => 2,
            MessageKind::Abort => 3,
            MessageKind::InviteIdentity => 4,
            MessageKind::InviteBundle => 5,
        }
    }

    /// Parses the wire byte.
    pub fn from_byte(byte: u8) -> Result<Self, KeysError> {
        match byte {
            1 => Ok(MessageKind::Identity),
            2 => Ok(MessageKind::Bundle),
            3 => Ok(MessageKind::Abort),
            4 => Ok(MessageKind::InviteIdentity),
            5 => Ok(MessageKind::InviteBundle),
            other => Err(KeysError::Corrupted(format!(
                "unknown pairing message kind {other}"
            ))),
        }
    }
}

/// The 12-character code the joiner shows and the sponsor types: 48 bits of mailbox
/// id and a 12-bit checksum bound to the server address (`PAIRING_CODE_DOMAIN`).
///
/// The checksum covers the canonical form of `server_url` (surrounding whitespace and
/// trailing slashes removed, ASCII letters lowercased), so `https://Sync.example.com/`
/// and `https://sync.example.com` give the same code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairingCode {
    /// The mailbox on the server.
    pub mailbox_id: [u8; MAILBOX_ID_LEN],
}

impl PairingCode {
    /// Crockford base32, `PAIRING_CODE_LEN` characters, upper case, no separators.
    #[must_use]
    pub fn encode(&self, server_url: &str) -> String {
        let id = self
            .mailbox_id
            .iter()
            .fold(0u64, |acc, &byte| (acc << 8) | u64::from(byte));
        let value = (id << CHECKSUM_BITS) | u64::from(checksum(&self.mailbox_id, server_url));
        (0..PAIRING_CODE_LEN)
            .rev()
            .map(|position| symbol(value >> (position * SYMBOL_BITS)))
            .collect()
    }

    /// [`Self::encode`] as the UI shows it: three groups of four separated by spaces.
    #[must_use]
    pub fn display(&self, server_url: &str) -> String {
        let code = self.encode(server_url);
        code.as_bytes()
            .chunks(4)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Parses a typed code (case-insensitive, spaces and dashes ignored, `O`→`0`,
    /// `I`/`L`→`1`) and checks it against `server_url`; a checksum mismatch is
    /// [`KeysError::InvalidKit`]-like: [`KeysError::Corrupted`] with a user-safe
    /// message ("this code is not for this server, or it was mistyped").
    ///
    /// Whitespace of any kind and the dash characters (`-`, U+2010 to U+2015, U+2212)
    /// are separators; `U` and every other character are refused. The error never
    /// repeats the typed text.
    pub fn decode(text: &str, server_url: &str) -> Result<Self, KeysError> {
        let wrong_length = || {
            KeysError::Corrupted(format!(
                "a pairing code has {PAIRING_CODE_LEN} letters and digits"
            ))
        };
        let mut value = 0u64;
        let mut symbols = 0usize;
        for c in text.chars() {
            if c.is_whitespace() || is_dash(c) {
                continue;
            }
            let digit = symbol_value(c).ok_or_else(|| {
                KeysError::Corrupted(
                    "a pairing code has only digits and the letters A to Z, without U".to_owned(),
                )
            })?;
            symbols += 1;
            if symbols > PAIRING_CODE_LEN {
                return Err(wrong_length());
            }
            value = (value << SYMBOL_BITS) | u64::from(digit);
        }
        if symbols != PAIRING_CODE_LEN {
            return Err(wrong_length());
        }
        let [_, _, a, b, c, d, e, f] = (value >> CHECKSUM_BITS).to_be_bytes();
        let mailbox_id = [a, b, c, d, e, f];
        let typed_checksum = value & ((1 << CHECKSUM_BITS) - 1);
        if typed_checksum != u64::from(checksum(&mailbox_id, server_url)) {
            return Err(KeysError::Corrupted(
                "this code is not for this server, or it was mistyped".to_owned(),
            ));
        }
        Ok(Self { mailbox_id })
    }

    /// The mailbox id as 12 lowercase hex characters (URLs).
    #[must_use]
    pub fn mailbox_hex(&self) -> String {
        self.mailbox_id.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Parses [`Self::mailbox_hex`]: exactly 12 hex digits (either case).
    pub fn from_mailbox_hex(text: &str) -> Result<Self, KeysError> {
        let malformed =
            || KeysError::Corrupted(format!("a mailbox id is {} hex digits", MAILBOX_ID_LEN * 2));
        let digits = text.as_bytes();
        if digits.len() != MAILBOX_ID_LEN * 2 {
            return Err(malformed());
        }
        let mut mailbox_id = [0u8; MAILBOX_ID_LEN];
        let (pairs, _) = digits.as_chunks::<2>();
        for (byte, &[high, low]) in mailbox_id.iter_mut().zip(pairs) {
            let high = hex_value(high).ok_or_else(malformed)?;
            let low = hex_value(low).ok_or_else(malformed)?;
            *byte = (high << 4) | low;
        }
        Ok(Self { mailbox_id })
    }
}

/// The first 12 bits (big-endian) of `BLAKE3(PAIRING_CODE_DOMAIN || canonical
/// server_url || mailbox_id)` (`kb_crypto::Hasher` gives the digest of the
/// concatenation). The domain has a fixed value and the id a fixed length, so the
/// input is unambiguous.
fn checksum(mailbox_id: &[u8; MAILBOX_ID_LEN], server_url: &str) -> u16 {
    let mut hasher = kb_crypto::Hasher::new();
    hasher.update(PAIRING_CODE_DOMAIN.as_bytes());
    hasher.update(canonical_server_url(server_url).as_bytes());
    hasher.update(mailbox_id);
    let [first, second, ..] = hasher.finalize();
    (u16::from(first) << 4) | (u16::from(second) >> 4)
}

/// The server address as the code checksum covers it: surrounding whitespace and
/// trailing slashes removed, ASCII letters lowercased (what `sync_configure` stores,
/// lowercased; non-ASCII characters are kept as they are).
fn canonical_server_url(server_url: &str) -> String {
    server_url.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// The Crockford symbol of the low five bits of `value`.
fn symbol(value: u64) -> char {
    usize::try_from(value & 0x1f)
        .ok()
        .and_then(|index| CROCKFORD.get(index))
        .map_or('0', |&byte| char::from(byte))
}

/// The value of a typed symbol: case-insensitive, `O` read as `0`, `I` and `L` as `1`.
pub(crate) fn symbol_value(c: char) -> Option<u8> {
    let c = match c.to_ascii_uppercase() {
        'O' => '0',
        'I' | 'L' => '1',
        other => other,
    };
    CROCKFORD
        .iter()
        .position(|&s| char::from(s) == c)
        .and_then(|index| u8::try_from(index).ok())
}

/// ASCII hyphen-minus and the Unicode dashes a phone keyboard or a copy may produce.
pub(crate) fn is_dash(c: char) -> bool {
    matches!(c, '-' | '\u{2010}'..='\u{2015}' | '\u{2212}')
}

fn hex_value(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

/// The joiner's identity, sent after the joiner confirmed the digits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// The joining device.
    pub device_id: DeviceId,
    /// Its public keys.
    pub keys: DevicePublicKeys,
    /// Its name (padded to `DEVICE_NAME_PAD` on the wire).
    pub name: String,
}

impl Identity {
    /// Strict length-prefixed encoding, always 148 bytes whatever the name (layout in
    /// the [module documentation](self)). A name longer than `DEVICE_NAME_PAD` bytes
    /// is cut at the last character boundary that fits.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let name = truncate_utf8(&self.name, DEVICE_NAME_PAD);
        let mut out = Vec::with_capacity(IDENTITY_LEN);
        out.extend_from_slice(self.device_id.as_bytes());
        out.extend_from_slice(&self.keys.signing.to_bytes());
        out.extend_from_slice(&self.keys.agreement.to_bytes());
        put_bytes(&mut out, name.as_bytes());
        out.resize(IDENTITY_LEN, 0);
        out
    }

    /// Parses [`Self::encode`]; anything else (another length, a name longer than
    /// the pad, non-zero padding, a name that is not UTF-8, an invalid public key) is
    /// [`KeysError::Corrupted`].
    pub fn decode(bytes: &[u8]) -> Result<Self, KeysError> {
        let bad = |error: DecodeError| malformed("identity", error.0);
        let mut r = Reader::new(bytes);
        let device_id = DeviceId::from_bytes(r.array().map_err(bad)?);
        let signing = VerifyingKey::from_bytes(r.take(32).map_err(bad)?)
            .map_err(|_| malformed("identity", "invalid signing key"))?;
        let agreement = AgreementPublic::from_bytes(r.take(AGREEMENT_PUBLIC_LEN).map_err(bad)?)
            .map_err(|_| malformed("identity", "invalid agreement key"))?;
        let name_len = usize::try_from(r.u32().map_err(bad)?)
            .ok()
            .filter(|&len| len <= DEVICE_NAME_PAD)
            .ok_or_else(|| malformed("identity", "name longer than the pad"))?;
        let field = r.take(DEVICE_NAME_PAD).map_err(bad)?;
        r.finish().map_err(bad)?;
        let (name, padding) = field
            .split_at_checked(name_len)
            .ok_or_else(|| malformed("identity", "name longer than the pad"))?;
        if padding.iter().any(|&byte| byte != 0) {
            return Err(malformed("identity", "non-zero padding"));
        }
        let name = core::str::from_utf8(name)
            .map_err(|_| malformed("identity", "the name is not UTF-8"))?
            .to_owned();
        Ok(Self {
            device_id,
            keys: DevicePublicKeys { signing, agreement },
            name,
        })
    }
}

/// What the invitee sends the inviter after it confirmed the digits of an invite session (ADR-0046,
/// `MessageKind::InviteIdentity`): who it is, in a form the inviter can check against the account
/// id, and the name it wants to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteIdentity {
    /// The invitee's account.
    pub account_id: AccountId,
    /// `G`: the BLAKE3 of the genesis entry of the account's device log (`safety::genesis_hash`);
    /// the account id is its first 16 bytes, and both safety numbers are computed from it.
    pub genesis_hash: [u8; 32],
    /// The head of the invitee's device log.
    pub log_head: crate::membership::ChainHead,
    /// The whole encoded device log when the account is not on the server yet (at most
    /// `kb_types::sharing::INVITE_BUNDLE_LOG_MAX_BYTES` in all), so the inviter can check that it
    /// hashes to the account id; `None` when the server has it.
    pub log: Option<Vec<Vec<u8>>>,
    /// The display name (1 to `PROFILE_NAME_MAX_CHARS` characters).
    pub display_name: String,
}

/// Version byte of an encoded [`InviteIdentity`].
const INVITE_IDENTITY_VERSION: u8 = 1;

impl InviteIdentity {
    /// Strict length-prefixed encoding, little-endian:
    ///
    /// ```text
    /// u8 version = 1 | [16] account | [32] genesis hash | u64 head seq | [32] head hash |
    /// u8 has_log (0 or 1) | if 1: u32 count, count x (u32 len + encoded log entry) |
    /// u32 len + display name (UTF-8)
    /// ```
    ///
    /// Nothing is cut: the caller keeps `log` within `INVITE_BUNDLE_LOG_MAX_BYTES` (a log that is
    /// bigger is not sent; the invitee is then expected to be on the server already), and the name
    /// within `PROFILE_NAME_MAX_CHARS`; [`Self::decode`] refuses both.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + 16 + 32 + 8 + 32 + 1 + 4 + self.display_name.len());
        out.push(INVITE_IDENTITY_VERSION);
        out.extend_from_slice(self.account_id.as_bytes());
        out.extend_from_slice(&self.genesis_hash);
        out.extend_from_slice(&self.log_head.seq.to_le_bytes());
        out.extend_from_slice(&self.log_head.hash);
        match &self.log {
            None => out.push(0),
            Some(entries) => {
                out.push(1);
                let count = u32::try_from(entries.len()).unwrap_or(u32::MAX);
                out.extend_from_slice(&count.to_le_bytes());
                for entry in entries
                    .iter()
                    .take(usize::try_from(count).unwrap_or(usize::MAX))
                {
                    put_bytes(&mut out, entry);
                }
            }
        }
        put_bytes(&mut out, self.display_name.as_bytes());
        out
    }

    /// Parses [`Self::encode`]; truncation, trailing bytes, a log over the size limit and a name
    /// that is not UTF-8, empty or longer than `PROFILE_NAME_MAX_CHARS` characters are
    /// [`KeysError::Corrupted`] (static text, never the bytes).
    pub fn decode(bytes: &[u8]) -> Result<Self, KeysError> {
        let bad = |error: DecodeError| malformed("invite identity", error.0);
        let mut r = Reader::new(bytes);
        let version = r.u8().map_err(bad)?;
        if version != INVITE_IDENTITY_VERSION {
            return Err(malformed("invite identity", "unknown version"));
        }
        let account_id = AccountId::from_bytes(r.array().map_err(bad)?);
        let genesis_hash = r.array().map_err(bad)?;
        let log_head = crate::membership::ChainHead {
            seq: r.u64().map_err(bad)?,
            hash: r.array().map_err(bad)?,
        };
        let log = if r.bool().map_err(bad)? {
            let count = r.u32().map_err(bad)?;
            let mut entries = Vec::new();
            let mut total = 0usize;
            for _ in 0..count {
                let entry = r.bytes().map_err(bad)?;
                total = total.saturating_add(entry.len());
                if total > kb_types::sharing::INVITE_BUNDLE_LOG_MAX_BYTES {
                    return Err(malformed("invite identity", "the log is too big"));
                }
                entries.push(entry.to_vec());
            }
            Some(entries)
        } else {
            None
        };
        let name = core::str::from_utf8(r.bytes().map_err(bad)?)
            .map_err(|_| malformed("invite identity", "the name is not UTF-8"))?
            .to_owned();
        r.finish().map_err(bad)?;
        let chars = name.chars().count();
        if chars == 0 || chars > kb_types::sharing::PROFILE_NAME_MAX_CHARS {
            return Err(malformed(
                "invite identity",
                "the name has the wrong length",
            ));
        }
        Ok(Self {
            account_id,
            genesis_hash,
            log_head,
            log,
            display_name: name,
        })
    }
}

/// The longest prefix of `text` of at most `max` bytes that ends on a character
/// boundary.
fn truncate_utf8(text: &str, max: usize) -> &str {
    let end = (0..=max.min(text.len()))
        .rev()
        .find(|&end| text.is_char_boundary(end))
        .unwrap_or(0);
    text.get(..end).unwrap_or_default()
}

/// What the sponsor hands over after both confirmations.
#[derive(Clone)]
pub struct Bundle {
    /// The account root key.
    pub ark: Ark,
    /// Its generation.
    pub ark_generation: u32,
    /// The account.
    pub account_id: AccountId,
    /// The sync server.
    pub server_url: String,
    /// The whole device log, encoded entries in order (the joiner's `DeviceAdded`
    /// included).
    pub log: Vec<Vec<u8>>,
    /// The recovery lookup id.
    pub recovery_id: [u8; 32],
    /// The recovery blob as stored (`recovery::RecoveryBlob::encode`).
    pub recovery_blob: Vec<u8>,
    /// The workspace.
    pub workspace_id: WorkspaceId,
    /// Its name.
    pub workspace_name: String,
}

impl Bundle {
    /// Strict length-prefixed encoding (the root key bytes included: only ever
    /// sealed). Layout in the [module documentation](self). The buffer is allocated
    /// once at its final size, so no reallocation leaves a copy of the root key
    /// behind, and it is wiped when dropped.
    #[must_use]
    pub fn encode(&self) -> Zeroizing<Vec<u8>> {
        let prefixed = |len: usize| len.saturating_add(4);
        let len = [
            32 + 4 + 16,
            prefixed(self.server_url.len()),
            4,
            self.log.iter().fold(0usize, |sum, entry| {
                sum.saturating_add(prefixed(entry.len()))
            }),
            32,
            prefixed(self.recovery_blob.len()),
            16,
            prefixed(self.workspace_name.len()),
        ]
        .iter()
        .fold(0usize, |sum, part| sum.saturating_add(*part));

        let mut out = Zeroizing::new(Vec::with_capacity(len));
        out.extend_from_slice(self.ark.secret().expose());
        out.extend_from_slice(&self.ark_generation.to_le_bytes());
        out.extend_from_slice(self.account_id.as_bytes());
        put_bytes(&mut out, self.server_url.as_bytes());
        let count = u32::try_from(self.log.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&count.to_le_bytes());
        for entry in self
            .log
            .iter()
            .take(usize::try_from(count).unwrap_or(usize::MAX))
        {
            put_bytes(&mut out, entry);
        }
        out.extend_from_slice(&self.recovery_id);
        put_bytes(&mut out, &self.recovery_blob);
        out.extend_from_slice(self.workspace_id.as_bytes());
        put_bytes(&mut out, self.workspace_name.as_bytes());
        out
    }

    /// Parses [`Self::encode`]; truncation, trailing bytes and strings that are not
    /// UTF-8 are [`KeysError::Corrupted`]. The log entries, the recovery blob and the
    /// server address are returned as they are: the caller verifies the log.
    pub fn decode(bytes: &[u8]) -> Result<Self, KeysError> {
        let bad = |error: DecodeError| malformed("bundle", error.0);
        let text = |raw: &[u8], what: &'static str| {
            core::str::from_utf8(raw)
                .map(str::to_owned)
                .map_err(|_| malformed("bundle", what))
        };
        let mut r = Reader::new(bytes);
        let ark_bytes = Zeroizing::new(r.array::<32>().map_err(bad)?);
        let ark = Ark::from_secret(Secret32::from_bytes(*ark_bytes));
        let ark_generation = r.u32().map_err(bad)?;
        let account_id = AccountId::from_bytes(r.array().map_err(bad)?);
        let server_url = text(r.bytes().map_err(bad)?, "the server address is not UTF-8")?;
        let count = r.u32().map_err(bad)?;
        // Every entry takes at least its 4-byte length, so a bogus count runs out of
        // input quickly; nothing is reserved up front.
        let mut log = Vec::new();
        for _ in 0..count {
            log.push(r.bytes().map_err(bad)?.to_vec());
        }
        let recovery_id = r.array().map_err(bad)?;
        let recovery_blob = r.bytes().map_err(bad)?.to_vec();
        let workspace_id = WorkspaceId::from_bytes(r.array().map_err(bad)?);
        let workspace_name = text(r.bytes().map_err(bad)?, "the workspace name is not UTF-8")?;
        r.finish().map_err(bad)?;
        Ok(Self {
            ark,
            ark_generation,
            account_id,
            server_url,
            log,
            recovery_id,
            recovery_blob,
            workspace_id,
            workspace_name,
        })
    }
}

/// `KeysError::Corrupted("malformed pairing <what>: <reason>")`; static text only.
fn malformed(what: &str, reason: &str) -> KeysError {
    KeysError::Corrupted(format!("malformed pairing {what}: {reason}"))
}

/// One side of a pairing: the ephemeral key, the joiner's commitment (on the sponsor,
/// until the reveal), the transcript and the channel key once both ephemeral keys are
/// known.
///
/// Nothing in it is printable: no `Debug`, and errors never carry key material or
/// the digits.
pub struct Session {
    role: Role,
    /// The transcript domain: [`PAIRING_TRANSCRIPT_DOMAIN`] for a device pairing,
    /// `kb_types::sharing::INVITE_TRANSCRIPT_DOMAIN` for an invite. It is part of the transcript the
    /// channel key and the SAS are derived from, so a message of one kind of session never opens in
    /// the other.
    domain: &'static str,
    /// Known from the start on the sponsor; set by [`Session::set_mailbox`] on the
    /// joiner once the server answered the offer (before `accept_answer`).
    mailbox_id: Option<[u8; MAILBOX_ID_LEN]>,
    ephemeral: AgreementSecret,
    /// Sponsor only: the commitment the offer carried, checked by the reveal.
    commitment: Option<[u8; 32]>,
    peer: Option<AgreementPublic>,
    channel_key: Option<Secret32>,
    /// The SAS as a number below `SAS_MODULUS`, once agreed.
    sas: Option<u32>,
}

impl Session {
    /// A joiner session and its clear offer (`version u8 || role u8 || H [32]`, `H =
    /// BLAKE3("atomik-kb/v1/pairing-commit" || e_joiner)`). The mailbox does not
    /// exist yet: [`Session::set_mailbox`] follows.
    #[must_use]
    pub fn joiner(rng: &dyn Rng) -> (Self, Vec<u8>) {
        Self::opener(PAIRING_TRANSCRIPT_DOMAIN, rng)
    }

    /// The opener of a session over the transcript `domain`, and its clear offer: what
    /// [`Session::joiner`] does for device pairing and what the inviter does for an invite
    /// (`kb_types::sharing::INVITE_TRANSCRIPT_DOMAIN`).
    #[must_use]
    pub fn opener(domain: &'static str, rng: &dyn Rng) -> (Self, Vec<u8>) {
        let session = Self::new(Role::Opener, domain, None, rng);
        let offer = frame(Role::Opener, &commitment(&session.ephemeral.public()));
        (session, offer)
    }

    /// The joiner learns its mailbox id from the server's answer to the offer; it
    /// must be set before [`Session::accept_answer`] (the transcript needs it).
    /// Ignored once the agreement completed: the transcript is fixed by then.
    pub fn set_mailbox(&mut self, mailbox_id: [u8; MAILBOX_ID_LEN]) {
        if !self.agreed() {
            self.mailbox_id = Some(mailbox_id);
        }
    }

    /// A sponsor session from the joiner's offer, and the clear answer (`version u8
    /// || role u8 || e_sponsor [32]`). The sponsor keeps the commitment and agrees
    /// only on the reveal ([`Session::accept_reveal`]).
    ///
    /// An offer of the wrong length, version or role is [`KeysError::Corrupted`].
    pub fn sponsor(
        mailbox_id: [u8; MAILBOX_ID_LEN],
        offer: &[u8],
        rng: &dyn Rng,
    ) -> Result<(Self, Vec<u8>), KeysError> {
        Self::typist(PAIRING_TRANSCRIPT_DOMAIN, mailbox_id, offer, rng)
    }

    /// The typist of a session over the transcript `domain`, from the opener's offer, and the clear
    /// answer: what [`Session::sponsor`] does for device pairing and what the invitee does for an
    /// invite. The typist agrees only on the reveal ([`Session::accept_reveal`]).
    ///
    /// An offer of the wrong length, version or role is [`KeysError::Corrupted`].
    pub fn typist(
        domain: &'static str,
        mailbox_id: [u8; MAILBOX_ID_LEN],
        offer: &[u8],
        rng: &dyn Rng,
    ) -> Result<(Self, Vec<u8>), KeysError> {
        let commitment = parse_frame(offer, Role::Opener, "offer")?;
        let mut session = Self::new(Role::Typist, domain, Some(mailbox_id), rng);
        session.commitment = Some(commitment);
        let answer = frame(Role::Typist, &session.ephemeral.public().to_bytes());
        Ok((session, answer))
    }

    /// The joiner takes the sponsor's answer and returns its clear reveal (`version
    /// u8 || role u8 || e_joiner [32]`, the next message to post); the channel key
    /// and the SAS exist afterwards.
    ///
    /// [`KeysError::Corrupted`] on a sponsor session, before [`Session::set_mailbox`],
    /// after an answer was already taken, and for an answer of the wrong length,
    /// version or role; an invalid public key, or the joiner's own key reflected
    /// back, is [`KeysError::Crypto`]. A refused answer leaves the session as it was.
    pub fn accept_answer(&mut self, answer: &[u8]) -> Result<Vec<u8>, KeysError> {
        if self.role != Role::Opener {
            return Err(KeysError::Corrupted(
                "only the joiner takes a pairing answer".to_owned(),
            ));
        }
        if self.agreed() {
            return Err(KeysError::Corrupted(
                "the pairing answer was already taken".to_owned(),
            ));
        }
        if self.mailbox_id.is_none() {
            return Err(not_in_mailbox());
        }
        let sponsor_key =
            AgreementPublic::from_bytes(&parse_frame(answer, Role::Typist, "answer")?)?;
        self.agree(sponsor_key)?;
        Ok(frame(Role::Opener, &self.ephemeral.public().to_bytes()))
    }

    /// The sponsor takes the joiner's reveal: the key must be the one the offer
    /// committed to. The channel key and the SAS exist afterwards.
    ///
    /// [`KeysError::Corrupted`] on a joiner session, after a reveal was already
    /// taken, and for a reveal of the wrong length, version or role; an invalid
    /// public key is `Crypto(Encoding)`, and a key that does not match the commitment
    /// (or is the sponsor's own) is `Crypto(Authentication)`. A refused reveal leaves
    /// the session as it was.
    pub fn accept_reveal(&mut self, reveal: &[u8]) -> Result<(), KeysError> {
        if self.role != Role::Typist {
            return Err(KeysError::Corrupted(
                "only the sponsor takes a pairing reveal".to_owned(),
            ));
        }
        if self.agreed() {
            return Err(KeysError::Corrupted(
                "the pairing reveal was already taken".to_owned(),
            ));
        }
        let expected = self
            .commitment
            .ok_or_else(|| KeysError::Corrupted("the pairing offer is not known yet".to_owned()))?;
        let joiner_key =
            AgreementPublic::from_bytes(&parse_frame(reveal, Role::Opener, "reveal")?)?;
        if commitment(&joiner_key) != expected {
            return Err(KeysError::Crypto(CryptoError::Authentication));
        }
        self.agree(joiner_key)
    }

    /// This side.
    #[must_use]
    pub fn role(&self) -> Role {
        self.role
    }

    /// The mailbox, once known.
    #[must_use]
    pub fn mailbox_id(&self) -> Option<[u8; MAILBOX_ID_LEN]> {
        self.mailbox_id
    }

    /// Whether the key agreement completed (the SAS can be shown): on the joiner
    /// after [`Session::accept_answer`], on the sponsor after
    /// [`Session::accept_reveal`].
    #[must_use]
    pub fn agreed(&self) -> bool {
        self.channel_key.is_some()
    }

    /// The six digits both sides compare (`HKDF(ss, PAIRING_SAS, transcript)`, first
    /// four bytes big-endian modulo 1 000 000, zero-padded); `None` before the
    /// agreement.
    #[must_use]
    pub fn sas(&self) -> Option<String> {
        self.sas.map(|n| format!("{n:0SAS_DIGITS$}"))
    }

    /// Seals a message under the channel key (`EnvelopeKind::Pairing`, header `role
    /// u8 || seq u32 || kind u8`); empty before the agreement.
    ///
    /// `seq` is the mailbox sequence number the message gets (3 for the identity, 4
    /// for the bundle, see the [module documentation](self)); the receiver passes the
    /// same number to [`Session::open`].
    #[must_use]
    pub fn seal(&self, kind: MessageKind, seq: u32, plaintext: &[u8], rng: &dyn Rng) -> Vec<u8> {
        let Some(key) = &self.channel_key else {
            return Vec::new();
        };
        kb_crypto::seal_envelope(
            EnvelopeKind::Pairing,
            &header(self.role, seq, kind),
            key,
            plaintext,
            None,
            rng,
        )
    }

    /// Opens a message from the other side; a wrong role, sequence or key is
    /// [`KeysError::Crypto`].
    ///
    /// Precisely: malformed bytes, another envelope kind, a signature or a header of
    /// another shape are `Crypto(Encoding)`; a wrong key, tampering, a message this
    /// side sealed (reflected back) or another sequence number than `expected_seq`
    /// are `Crypto(Authentication)`; an unknown message kind from the peer is
    /// [`KeysError::Corrupted`], and so is a call before the agreement.
    pub fn open(
        &self,
        bytes: &[u8],
        expected_seq: u32,
    ) -> Result<(MessageKind, Zeroizing<Vec<u8>>), KeysError> {
        let key = self.channel_key.as_ref().ok_or_else(|| {
            KeysError::Corrupted("the pairing channel is not open yet".to_owned())
        })?;
        let (envelope, plaintext) = kb_crypto::open_envelope(bytes, key, None)?;
        if envelope.kind != EnvelopeKind::Pairing || envelope.signature.is_some() {
            return Err(KeysError::Crypto(CryptoError::Encoding(
                "not a pairing message".to_owned(),
            )));
        }
        let Ok([role, s0, s1, s2, s3, kind]) =
            <[u8; HEADER_LEN]>::try_from(envelope.header.as_slice())
        else {
            return Err(KeysError::Crypto(CryptoError::Encoding(
                "malformed pairing message header".to_owned(),
            )));
        };
        if role != self.role.peer().to_byte()
            || u32::from_le_bytes([s0, s1, s2, s3]) != expected_seq
        {
            return Err(KeysError::Crypto(CryptoError::Authentication));
        }
        Ok((MessageKind::from_byte(kind)?, plaintext))
    }

    /// A session with a fresh ephemeral key from `rng`.
    fn new(
        role: Role,
        domain: &'static str,
        mailbox_id: Option<[u8; MAILBOX_ID_LEN]>,
        rng: &dyn Rng,
    ) -> Self {
        Self {
            role,
            domain,
            mailbox_id,
            ephemeral: AgreementSecret::from_seed(&Secret32::generate(rng)),
            commitment: None,
            peer: None,
            channel_key: None,
            sas: None,
        }
    }

    /// Derives the channel key and the SAS from the peer's ephemeral key; on error
    /// nothing changes.
    fn agree(&mut self, peer: AgreementPublic) -> Result<(), KeysError> {
        let mailbox_id = self.mailbox_id.ok_or_else(not_in_mailbox)?;
        let mine = self.ephemeral.public();
        if peer == mine {
            // Our own key reflected by the relay: nobody would hold the other half.
            return Err(KeysError::Crypto(CryptoError::Authentication));
        }
        let shared = kb_crypto::shared_secret(&self.ephemeral, &peer)?;
        let (joiner, sponsor) = match self.role {
            Role::Opener => (mine, peer),
            Role::Typist => (peer, mine),
        };
        let transcript = transcript(self.domain, &mailbox_id, &joiner, &sponsor);
        let channel_key = kb_crypto::derive_key(&shared, Label::PAIRING_CHANNEL, &transcript);
        let sas_key = kb_crypto::derive_key(&shared, Label::PAIRING_SAS, &transcript);
        let [a, b, c, d, ..] = *sas_key.expose();
        self.sas = Some(u32::from_be_bytes([a, b, c, d]) % SAS_MODULUS);
        self.channel_key = Some(channel_key);
        self.peer = Some(peer);
        self.commitment = None;
        Ok(())
    }
}

fn not_in_mailbox() -> KeysError {
    KeysError::Corrupted("the pairing mailbox is not known yet".to_owned())
}

/// `BLAKE3(PAIRING_COMMIT_DOMAIN || e_joiner)`.
fn commitment(joiner: &AgreementPublic) -> [u8; 32] {
    let mut hasher = kb_crypto::Hasher::new();
    hasher.update(PAIRING_COMMIT_DOMAIN.as_bytes());
    hasher.update(&joiner.to_bytes());
    hasher.finalize()
}

/// `PAIRING_TRANSCRIPT_DOMAIN || mailbox_id || e_joiner || e_sponsor` (fixed-length
/// fields after a fixed domain: unambiguous).
fn transcript(
    domain: &str,
    mailbox_id: &[u8; MAILBOX_ID_LEN],
    joiner: &AgreementPublic,
    sponsor: &AgreementPublic,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(domain.len() + MAILBOX_ID_LEN + 2 * AGREEMENT_PUBLIC_LEN);
    out.extend_from_slice(domain.as_bytes());
    out.extend_from_slice(mailbox_id);
    out.extend_from_slice(&joiner.to_bytes());
    out.extend_from_slice(&sponsor.to_bytes());
    out
}

/// `role u8 || seq u32 LE || kind u8`.
fn header(role: Role, seq: u32, kind: MessageKind) -> [u8; HEADER_LEN] {
    let [a, b, c, d] = seq.to_le_bytes();
    [role.to_byte(), a, b, c, d, kind.to_byte()]
}

/// A clear message: `version u8 || role u8 || payload [32]`.
fn frame(from: Role, payload: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HELLO_LEN);
    out.push(PAIRING_VERSION);
    out.push(from.to_byte());
    out.extend_from_slice(payload);
    out
}

/// Parses a clear message `what` (offer, answer, reveal) that must come from `from`,
/// returning its 32-byte payload.
fn parse_frame(bytes: &[u8], from: Role, what: &str) -> Result<[u8; 32], KeysError> {
    let Ok([version, role, payload @ ..]) = <[u8; HELLO_LEN]>::try_from(bytes) else {
        return Err(malformed(what, "wrong length"));
    };
    if version != PAIRING_VERSION {
        return Err(KeysError::Corrupted(format!(
            "unsupported pairing version {version}"
        )));
    }
    if role != from.to_byte() {
        return Err(malformed(what, "it comes from the wrong side"));
    }
    Ok(payload)
}

#[cfg(test)]
mod tests;
