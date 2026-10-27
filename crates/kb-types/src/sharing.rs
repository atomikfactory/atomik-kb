//! Shared spaces (Milestone 13, ADR-0041 to ADR-0051, `docs/security/sharing.md`,
//! `docs/development/milestone-13.md`, "Shared constants" and slice 1, section 1.1): the constants of
//! the feature and the plain data of its API (roles, member lists, invites, profiles, people).
//! Pure data; the membership log and its rules live in `kb-keys`, the wire protocol in `kb-sync`,
//! the behaviour in `kb-core`.
//!
//! The constants are not glob re-exported: refer to them as `kb_types::sharing::...`. The types are
//! re-exported at the crate root like every other API type.
//!
//! JSON shape: structs are camelCase; the enums of states keep their variant names (like
//! `PairingState`); [`MemberRole`] is lowercase (like `PageFont`). Optional fields are `Option<T>`
//! left out when absent and optional in TypeScript.

use serde::{Deserialize, Serialize};

use crate::ids::{AccountId, DeviceId, FeedId, InviteId, LinkId, PageId, SpaceId};

// ----- constants ----------------------------------------------------------------------------

/// Members one shared feed holds at most (the server refuses the Join past it).
pub const MAX_MEMBERS_PER_FEED: usize = 50;
/// Feeds one account belongs to at most.
pub const MAX_FEEDS_PER_ACCOUNT: usize = 100;
/// Boxes (space key sealed to one device each) one membership entry carries at most.
pub const MAX_BOXES_PER_ENTRY: usize = 1024;
/// Open invites one space holds at most.
pub const MAX_OPEN_INVITES: usize = 20;
/// The longest an invite lives, in days.
pub const INVITE_MAX_DAYS: u32 = 30;
/// How long a link invite lives when the Owner does not say, in days.
pub const INVITE_LINK_DEFAULT_DAYS: u32 = 7;
/// How long the `Invite` an inviter writes after the short authentication string lives, in seconds.
pub const INVITE_CODE_TTL_SECONDS: u64 = 3600;
/// Rekeys one account may post per feed per day.
pub const REKEYS_PER_ACCOUNT_PER_FEED_PER_DAY: u32 = 20;
/// `invites/{id}/open` calls per IP address per hour.
pub const INVITE_OPEN_PER_IP_PER_HOUR: u32 = 30;
/// Accounts a server lets share invites create, when the operator sets no limit.
pub const SHARE_SIGNUPS_DEFAULT: u32 = 20;
/// Accounts one inviting account's invites may create, when the operator sets no limit.
pub const SHARE_SIGNUPS_PER_ACCOUNT_DEFAULT: u32 = 5;
/// Days after a snapshot arrived before the entries it covers may be pruned (all members' devices must
/// also have acknowledged them, ADR-0045).
pub const PRUNE_ACK_DAYS: u32 = 7;
/// Days without a session after which a device no longer holds pruning back, when the operator sets no
/// `KB_SERVER_IDLE_DEVICE_DAYS`.
pub const PRUNE_IDLE_DEFAULT_DAYS: u32 = 30;
/// Days a closed feed stays readable before the server deletes it.
pub const CLOSE_READ_DAYS: u32 = 30;
/// The most a log may weigh inside an `InviteIdentity` message (the pairing message limit).
pub const INVITE_BUNDLE_LOG_MAX_BYTES: usize = 16 * 1024;
/// Characters of a profile name (1 to this many after trimming).
pub const PROFILE_NAME_MAX_CHARS: usize = 100;
/// Days a shared space keeps its trashed pages before the Owners' devices purge them, until an Owner
/// says otherwise (`SpaceSettings::trash_retention_days`; 0 keeps them).
pub const DEFAULT_SPACE_TRASH_RETENTION_DAYS: u32 = 30;
/// Domain of the safety number hash (ADR-0042).
pub const SAFETY_NUMBER_DOMAIN: &str = "atomik-kb/v1/safety-number";
/// Domain of the space key commitment (ADR-0044).
pub const SK_COMMIT_DOMAIN: &str = "atomik-kb/v1/sk-commit";
/// Context of a space key box; `/<feed hex>/<epoch>` follows (ADR-0044).
pub const SPACE_KEY_CONTEXT: &str = "atomik-kb/v1/space-key";
/// Transcript domain of the invite pairing session and context of invite signatures (ADR-0046).
pub const INVITE_TRANSCRIPT_DOMAIN: &str = "atomik-kb/v1/invite";

// ----- constants of slice 3: realtime and presence (ADR-0050) -------------------------------

/// The largest feed item (entry, keyring or membership entry) the server relays inline in a binary
/// WebSocket frame; a larger one is announced with a `changed` text frame and pulled.
pub const INLINE_ENTRY_MAX_BYTES: usize = 65_536;
/// The largest presence frame, counted as the whole binary WebSocket frame (`2 | feed | envelope`).
pub const PRESENCE_MAX_FRAME_BYTES: usize = 4096;
/// Presence frames the server relays per connection per second (the excess is dropped).
pub const PRESENCE_FRAMES_PER_SECOND: u32 = 10;
/// Presence updates a client sends per second (the UI's limit; one update is one frame).
pub const PRESENCE_UPDATES_PER_SECOND: u32 = 10;
/// Seconds without a frame after which a device's presence is gone (the receivers' rule).
pub const PRESENCE_TIMEOUT_SECONDS: u64 = 30;
/// The push debounce of the driver, in milliseconds, while a shared feed has a remote presence
/// (otherwise `kb_types::sync::PUSH_DEBOUNCE_MS`).
pub const PUSH_DEBOUNCE_PRESENCE_MS: u64 = 150;
/// The HKDF label of the presence key, `HKDF(SK[e], "presence", feed)` (`kb_crypto::Label::PRESENCE`).
pub const PRESENCE_LABEL: &str = "presence";
/// The largest `update` a presence frame carries, in bytes: what remains of
/// [`PRESENCE_MAX_FRAME_BYTES`] after the frame prefix, the envelope's header, tag and signature and
/// the bucket padding of the sealed body (`milestone-13.md`, slice 3 contract notes).
pub const PRESENCE_UPDATE_MAX_BYTES: usize = 3072;

// ----- constants of slice 4: view links and the web viewer (ADR-0051) ----------------------------

/// How long after a change to its page a member's device waits before it refreshes the page's view
/// links (the publication is made once for a burst of edits).
pub const LINK_REFRESH_DEBOUNCE_MS: u64 = 60_000;
/// Seconds between the viewer's polls of its publication (`If-None-Match: pub_seq`).
pub const LINK_POLL_SECONDS: u64 = 60;
/// Public link reads (`GET /v2/links/{id}` and `GET /v2/links/{id}/chunks/{hash}`) the server answers per
/// client address per minute.
pub const LINK_GETS_PER_MINUTE_PER_IP: u32 = 120;
/// The HKDF label of the link id, `link_id = HKDF(K_link, "link-id")[..16]` (`kb_crypto::Label::LINK_ID`).
pub const LINK_ID_LABEL: &str = "link-id";
/// The HKDF label of the publication key, `HKDF(K_link, "publication")` (`kb_crypto::Label::PUBLICATION`).
pub const PUBLICATION_LABEL: &str = "publication";
/// The `Content-Security-Policy` of `/view` (ADR-0051): the viewer's own scripts and WebAssembly, requests to
/// its own origin only, images from its own origin and from the blobs it decrypts. The same text is the
/// `<meta http-equiv>` policy of `apps/viewer/index.html`.
pub const VIEWER_CSP: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; img-src 'self' blob:";
/// The longest expiry a view link may be given, in days (`ViewLinkCreate::expires_in_days` is 1 to this;
/// absent means no expiry). Not in the spec: the contract's bound for `InvalidInput` on `expiresInDays`.
pub const LINK_EXPIRY_MAX_DAYS: u32 = 365;
/// The most chunk hashes one publication declares (`PUT /v2/links/{id}`), and so the most attachment
/// chunks a linked page may carry (4 MiB each): the core refuses to link a page past it (`InvalidInput`
/// for `pageId`), the server refuses a larger declaration. Not in the spec.
pub const LINK_MAX_CHUNKS: usize = 1024;
/// The path of the viewer on a server, outside `/v2`; a link's URL is `<server>` + this + `#` + the key.
pub const VIEWER_PATH: &str = "/view";

// ----- enums -------------------------------------------------------------------------------

/// What a member may do in a shared space (ADR-0041). Ordered `Viewer < Commenter < Editor < Owner`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum MemberRole {
    /// Reads.
    Viewer,
    /// Reads and comments (slice 2).
    Commenter,
    /// Edits structure, pages and attachments, and comments.
    Editor,
    /// Everything: members, roles, invites, settings, closing, purging, deleting chunks.
    Owner,
}

impl MemberRole {
    /// Commenter and up.
    #[must_use]
    pub const fn can_comment(self) -> bool {
        matches!(self, Self::Commenter | Self::Editor | Self::Owner)
    }

    /// Editor and up.
    #[must_use]
    pub const fn can_edit(self) -> bool {
        matches!(self, Self::Editor | Self::Owner)
    }

    /// Owner.
    #[must_use]
    pub const fn is_owner(self) -> bool {
        matches!(self, Self::Owner)
    }

    /// The lowercase name the wire carries (`viewer`, `commenter`, `editor`, `owner`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Commenter => "commenter",
            Self::Editor => "editor",
            Self::Owner => "owner",
        }
    }

    /// The byte of the role in membership entries and in the server's `feed_members` table:
    /// 1 viewer, 2 commenter, 3 editor, 4 owner. On disk and on the wire: never renumber.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        match self {
            Self::Viewer => 1,
            Self::Commenter => 2,
            Self::Editor => 3,
            Self::Owner => 4,
        }
    }

    /// Parses [`Self::to_byte`]; any other byte is `None`.
    #[must_use]
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Viewer),
            2 => Some(Self::Commenter),
            3 => Some(Self::Editor),
            4 => Some(Self::Owner),
            _ => None,
        }
    }
}

/// Where a space stands in sharing (`SpaceInfo::share_state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum SpaceShareState {
    /// Not shared: the account's own space.
    #[default]
    Personal,
    /// `SpaceShare` ran; the feed's `Create` is not accepted by the server yet.
    Pending,
    /// Shared: a feed and members.
    Shared,
    /// A read-only local copy of a space this account was removed from or that an Owner closed.
    Archived,
}

impl SpaceShareState {
    /// `skip_serializing_if` of the default.
    #[must_use]
    pub const fn is_personal(&self) -> bool {
        matches!(self, Self::Personal)
    }
}

/// Why a space is an archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ArchiveReason {
    /// This account was removed.
    Removed,
    /// An Owner closed the space.
    Closed,
}

/// How an invite is redeemed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum InviteKind {
    /// Bound to one account, written after the short authentication string matched.
    Code,
    /// Not bound: whoever opens the link first joins.
    Link,
}

/// Where an invite stands, as the member panel lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum InviteStatus {
    /// Appended locally, not yet accepted by the server.
    Pending,
    /// Open: can be redeemed.
    Open,
    /// A `Join` consumed it.
    Used,
    /// The server's clock passed its expiry (shown when the state is known; clients do not decide it).
    Expired,
    /// An `InviteRevoke`, or its creator lost the Owner role.
    Revoked,
}

/// The two ways to invite (`InviteSessionInfo::flow`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum InviteFlow {
    /// A typed code and a short authentication string both sides compare.
    Code,
    /// A link with the secret in its fragment.
    Link,
}

/// Which end of an invite session this device is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum InviteSide {
    /// The Owner who invites (code flow: the side that opens the mailbox and shows the code).
    Inviter,
    /// The person who joins (code flow: the side that types the code).
    Invitee,
}

/// Where an invite session stands (transitions in `milestone-13.md`, section 1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum InviteSessionState {
    /// No session.
    Idle,
    /// The mailbox is being opened (inviter), the code checked (invitee) or the invite opened (link).
    Starting,
    /// The code is shown; nobody answered yet (inviter).
    Waiting,
    /// The six digits are shown; this side has not confirmed yet.
    Confirming,
    /// This side confirmed; the other side's turn (the inviter shows who is being invited).
    PeerConfirming,
    /// The link's preview is shown: space, inviter, role, "not verified" (link flow, invitee).
    Previewing,
    /// The invite or the bundle is on its way.
    Transferring,
    /// Account, open, verification, Join and bootstrap (invitee).
    Joining,
    /// Finished: the Join was posted (invitee) or seen (inviter).
    Done,
    /// Failed (see `error`).
    Failed,
    /// Cancelled here or by the other side.
    Cancelled,
}

// ----- types -------------------------------------------------------------------------------

/// What the share dialog counts before anything is sent (`SpaceSharePlan`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SpaceSharePlan {
    /// The space.
    pub space_id: SpaceId,
    /// Live pages.
    pub page_count: u32,
    /// Trashed pages, which travel too.
    pub trashed_page_count: u32,
    /// Attachments.
    pub attachment_count: u32,
    /// Plaintext bytes of the attachments.
    pub attachment_bytes: u64,
    /// Estimated sealed size of the full-history snapshots.
    pub history_bytes: u64,
    /// Sync is configured.
    pub server_configured: bool,
    /// The account id exists (the recovery kit was created).
    pub has_account: bool,
    /// A display name is set.
    pub has_profile_name: bool,
    /// The space is shared already.
    pub already_shared: bool,
}

/// The settings an Owner keeps for a shared space (ADR-0043 `Settings` entry; encrypted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SpaceSettings {
    /// Days a trashed page stays before the Owners' devices purge it; 0 keeps for ever.
    pub trash_retention_days: u32,
    /// Editors may create view links (slice 4; enforced by honest clients only).
    pub editors_may_link: bool,
}

impl Default for SpaceSettings {
    fn default() -> Self {
        Self {
            trash_retention_days: DEFAULT_SPACE_TRASH_RETENTION_DAYS,
            editors_may_link: false,
        }
    }
}

/// An open snapshot dispute: a device published a snapshot that omits changes it should contain
/// (ADR-0045), shown to Owners.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DisputeInfo {
    /// The page, when the document is a page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub page_id: Option<PageId>,
    /// The member whose device published it.
    pub account_id: AccountId,
    /// When the dispute was posted, Unix ms.
    pub at: i64,
}

/// A person who is no longer a member of a space (removed, or left), with the last name the space knew
/// them by (ADR-0048, ADR-0049): the words "Former member (name)" of a comment's author, a Person value
/// and a mention are made of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct FormerMemberInfo {
    /// The account.
    pub account_id: AccountId,
    /// The last name the space knew them by.
    pub name: String,
}

/// A member as the panel lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct MemberInfo {
    /// The member's account.
    pub account_id: AccountId,
    /// The self-asserted profile name.
    pub name: String,
    /// Role.
    pub role: MemberRole,
    /// The `at` of the `Join` (of the `Create` for the creator), Unix ms.
    pub joined_at: i64,
    /// This account.
    pub is_me: bool,
    /// `verifiedPeople` holds the account (ADR-0042).
    pub verified: bool,
    /// 30 digits in six groups of five; absent for this account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub safety_number: Option<String>,
    /// Active devices in the member's verified device log.
    pub device_count: u32,
    /// The latest `DeviceAdded` with `via_kit` seen in the member's log, Unix ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub via_kit_at: Option<i64>,
    /// Items of this member quarantined on this device.
    pub refused_changes: u32,
    /// Another member has the same name.
    pub name_shared: bool,
}

/// An invite as the Owners' panel lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct InviteInfo {
    /// The invite.
    pub invite_id: InviteId,
    /// Code or link.
    pub kind: InviteKind,
    /// The role it grants (never Owner).
    pub role: MemberRole,
    /// The Owner who made it.
    pub created_by: AccountId,
    /// When it was written, Unix ms.
    pub created_at: i64,
    /// When it expires, Unix ms (the server's clock decides).
    pub expires_at: i64,
    /// The account a code invite is bound to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub bound_account: Option<AccountId>,
    /// Where it stands.
    pub status: InviteStatus,
    /// The account that used it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub used_by: Option<AccountId>,
}

/// The membership of a shared space as the member panel shows it (`SpaceMembers`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct MembersInfo {
    /// The space.
    pub space_id: SpaceId,
    /// Its feed.
    pub feed_id: FeedId,
    /// This account's role.
    pub my_role: MemberRole,
    /// The current space key epoch.
    pub epoch: u32,
    /// A `Leave` or a revocation waits for a Rekey.
    pub pending_rekey: bool,
    /// When the `Close` entry was written, Unix ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub closed_at: Option<i64>,
    /// Current members, in join order.
    pub members: Vec<MemberInfo>,
    /// People who were members and are not now, with their last names, in account order (any member
    /// reads it); a person who joined again is a member, not listed here.
    #[serde(default)]
    pub former_members: Vec<FormerMemberInfo>,
    /// Owners see every invite; others get an empty list.
    pub invites: Vec<InviteInfo>,
    /// The space's settings.
    pub settings: SpaceSettings,
    /// Open snapshot disputes (Owners).
    pub disputes: Vec<DisputeInfo>,
    /// Items quarantined in this feed on this device.
    pub refused_total: u32,
    /// An item of a non-member was quarantined: the server misbehaved.
    pub server_flagged: bool,
}

/// A link invite, returned once: the core keeps no copy of the secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct InviteLinkInfo {
    /// The invite.
    pub invite_id: InviteId,
    /// `atomik-kb://invite?server=<url>#<s base32>`.
    pub link: String,
    /// `https://<server>/join#<s base32>`.
    pub web_link: String,
    /// The role it grants.
    pub role: MemberRole,
    /// When it expires, Unix ms.
    pub expires_at: i64,
}

/// An invite session of either flow and either side as the dialogs show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct InviteSessionInfo {
    /// Code or link.
    pub flow: InviteFlow,
    /// This device's side.
    pub side: InviteSide,
    /// Where it stands.
    pub state: InviteSessionState,
    /// The inviter's code, three groups of four (code flow).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub code: Option<String>,
    /// The six digits both sides compare (code flow).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub sas: Option<String>,
    /// The other side's name once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub peer_name: Option<String>,
    /// The other side's safety number, known after the identity message (code flow).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub peer_safety_number: Option<String>,
    /// The space's name, from the bundle (invitee).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub space_name: Option<String>,
    /// The inviter's name, from the bundle (invitee).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub inviter_name: Option<String>,
    /// The role the invite grants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub role: Option<MemberRole>,
    /// The code flow after the digits matched; always `false` for a link.
    pub verified: bool,
    /// The server the session goes through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub server_url: Option<String>,
    /// Why it failed, safe to show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub error: Option<String>,
    /// When the mailbox or the invite expires, Unix ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub expires_at: Option<i64>,
}

/// This account's profile: the name it shows to co-members (`profile` in the AccountDoc).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ProfileInfo {
    /// The display name, absent until set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub display_name: Option<String>,
}

/// A person (an account) as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PersonInfo {
    /// The account.
    pub account_id: AccountId,
    /// The self-asserted name, as the latest space entry or profile says.
    pub name: String,
    /// The safety numbers were compared (`verifiedPeople`).
    pub verified: bool,
    /// 30 digits in six groups of five; absent for this account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub safety_number: Option<String>,
    /// This account.
    pub is_me: bool,
}

/// Where one feed stands on this device (`SyncInfo::feeds`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct FeedSyncInfo {
    /// The feed.
    pub feed_id: FeedId,
    /// The space, for a shared feed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub space_id: Option<SpaceId>,
    /// Local updates and membership entries not yet uploaded.
    pub pending_entries: u32,
    /// Items quarantined in this feed.
    pub refused: u32,
    /// The server's history of this feed does not match this device's: uploads are paused.
    pub forked: bool,
    /// The feed was closed by an Owner.
    pub closed: bool,
    /// An item of a non-member was quarantined: the server misbehaved.
    pub server_flagged: bool,
}

// ----- presence (slice 3) --------------------------------------------------------------------

/// What a member's device is doing in a page, as its presence frame says (ADR-0050). The variant
/// names are the wire form, like the other state enums.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum PresenceState {
    /// The page is open and read.
    Viewing,
    /// The member is typing, or has the editing selection in it.
    Editing,
    /// The window is open but untouched for a while.
    Idle,
}

impl PresenceState {
    /// The byte of the state in the sealed body of a presence frame: 1 viewing, 2 editing, 3 idle.
    /// On the wire: never renumber.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        match self {
            Self::Viewing => 1,
            Self::Editing => 2,
            Self::Idle => 3,
        }
    }

    /// Parses [`Self::to_byte`]; any other byte is `None`.
    #[must_use]
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Viewing),
            2 => Some(Self::Editing),
            3 => Some(Self::Idle),
            _ => None,
        }
    }
}

/// Where one other device of a member is now (`PresenceList`, and inside a [`PresenceMessage`]). Names
/// and colours come from membership, never from the frame (ADR-0050): the name is the member's
/// profile name, the colour their place in join order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PresenceInfo {
    /// The member's account.
    pub account_id: AccountId,
    /// The device that sent the frame.
    pub device_id: DeviceId,
    /// The member's name in the space.
    pub name: String,
    /// The member's place in join order, 0 for the first member (the creator); the UI takes it
    /// modulo its palette, so every device shows a member in the same colour.
    pub color: u8,
    /// The page the device is in; absent when it is in no page (an `Idle` device, or a document that
    /// is not a page of this space).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub page_id: Option<PageId>,
    /// What it is doing.
    pub state: PresenceState,
    /// When this device's latest frame was accepted, Unix ms on the receiver's clock (a sender's
    /// claim about time is never used).
    pub updated_at: i64,
}

/// One accepted presence frame as the shell's channel `presence_events` delivers it and as
/// `PresenceTransport` hands it to the editor: who and where (`info`) and the sender's `EphemeralStore`
/// update (the Loro cursors of anchor and head), at most [`PRESENCE_UPDATE_MAX_BYTES`] bytes. The
/// update is a JSON array of numbers on the wire (a `Vec<u8>`); `core-client` turns it into a
/// `Uint8Array`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PresenceMessage {
    /// Who sent it and where.
    pub info: PresenceInfo,
    /// The ephemeral-store update, opaque to the core and the shell.
    pub update: Vec<u8>,
}

// ----- view links (slice 4) ------------------------------------------------------------------

/// Where a view link stands on this device (ADR-0051). The variant names are the wire form, like the
/// other state enums.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ViewLinkStatus {
    /// The link exists but no publication reached the server yet (the first upload is pending): the
    /// URL does not work until it does.
    Publishing,
    /// A publication is on the server and the link has not expired.
    Live,
    /// The link's expiry passed: the server answers 404.
    Expired,
    /// The link was revoked (transiently visible between a revoke and the next list; a revoked link
    /// is gone from the structure's `links` map).
    Revoked,
}

/// One view link of a space, as the share dialog and the page menu list it (`ViewLinkList`, and inside
/// the answer of the other two requests). The key in `url` is a secret: show it to the people who may
/// copy it and never log it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ViewLinkInfo {
    /// The link (`HKDF(K_link, "link-id")[..16]`).
    pub link_id: LinkId,
    /// The page the link shows.
    pub page_id: PageId,
    /// `<server url>/view#<K_link, base64url without padding>` in the `ViewLinkCreate` answer only: the URL
    /// is shown once, so `ViewLinkList` and revoke answers carry the empty string. It is never stored or
    /// logged; the `Link` keyring record keeps `K_link` so member devices can refresh the publication.
    pub url: String,
    /// The member who created the link.
    pub created_by: AccountId,
    /// That member's name in the space (their last known name when they have left).
    pub created_by_name: String,
    /// When it was created, Unix ms.
    pub created_at: i64,
    /// When it stops working, Unix ms; absent for a link that never expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub expires_at: Option<i64>,
    /// When this device's latest upload of the publication succeeded, Unix ms; absent before the
    /// first one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub last_published_at: Option<i64>,
    /// Where the link stands.
    pub status: ViewLinkStatus,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn account(n: u8) -> AccountId {
        AccountId::from_bytes([n; 16])
    }

    #[test]
    fn constants_have_the_documented_values() {
        assert_eq!(MAX_MEMBERS_PER_FEED, 50);
        assert_eq!(MAX_FEEDS_PER_ACCOUNT, 100);
        assert_eq!(MAX_BOXES_PER_ENTRY, 1024);
        assert_eq!(MAX_OPEN_INVITES, 20);
        assert_eq!((INVITE_MAX_DAYS, INVITE_LINK_DEFAULT_DAYS), (30, 7));
        assert_eq!(INVITE_CODE_TTL_SECONDS, 3600);
        assert_eq!(REKEYS_PER_ACCOUNT_PER_FEED_PER_DAY, 20);
        assert_eq!(INVITE_OPEN_PER_IP_PER_HOUR, 30);
        assert_eq!(
            (SHARE_SIGNUPS_DEFAULT, SHARE_SIGNUPS_PER_ACCOUNT_DEFAULT),
            (20, 5)
        );
        assert_eq!(
            (PRUNE_ACK_DAYS, PRUNE_IDLE_DEFAULT_DAYS, CLOSE_READ_DAYS),
            (7, 30, 30)
        );
        assert_eq!(INVITE_BUNDLE_LOG_MAX_BYTES, 16384);
        assert_eq!(
            INVITE_BUNDLE_LOG_MAX_BYTES,
            crate::devices::PAIRING_MAX_MESSAGE_BYTES
        );
        assert_eq!(PROFILE_NAME_MAX_CHARS, 100);
        assert_eq!(SAFETY_NUMBER_DOMAIN, "atomik-kb/v1/safety-number");
        assert_eq!(SK_COMMIT_DOMAIN, "atomik-kb/v1/sk-commit");
        assert_eq!(SPACE_KEY_CONTEXT, "atomik-kb/v1/space-key");
        assert_eq!(INVITE_TRANSCRIPT_DOMAIN, "atomik-kb/v1/invite");
        const { assert!(INVITE_LINK_DEFAULT_DAYS <= INVITE_MAX_DAYS) };
    }

    #[test]
    fn roles_are_ordered_lowercase_and_carry_their_bytes() {
        let ascending = [
            MemberRole::Viewer,
            MemberRole::Commenter,
            MemberRole::Editor,
            MemberRole::Owner,
        ];
        assert!(ascending.windows(2).all(|pair| pair[0] < pair[1]));
        let names = ["viewer", "commenter", "editor", "owner"];
        for (index, (role, name)) in ascending.into_iter().zip(names).enumerate() {
            assert_eq!(role.as_str(), name);
            assert_eq!(serde_json::to_value(role).unwrap(), json!(name));
            assert_eq!(
                serde_json::from_value::<MemberRole>(json!(name)).unwrap(),
                role
            );
            let byte = u8::try_from(index + 1).unwrap();
            assert_eq!(role.to_byte(), byte);
            assert_eq!(MemberRole::from_byte(byte), Some(role));
        }
        for bad in [0u8, 5, 0x80, 0xff] {
            assert_eq!(MemberRole::from_byte(bad), None);
        }
        assert!(serde_json::from_value::<MemberRole>(json!("Owner")).is_err());
        assert!(serde_json::from_value::<MemberRole>(json!("admin")).is_err());
        assert!(!MemberRole::Viewer.can_comment() && !MemberRole::Viewer.can_edit());
        assert!(MemberRole::Commenter.can_comment() && !MemberRole::Commenter.can_edit());
        assert!(MemberRole::Editor.can_comment() && MemberRole::Editor.can_edit());
        assert!(MemberRole::Owner.can_comment() && MemberRole::Owner.can_edit());
        assert!(MemberRole::Owner.is_owner() && !MemberRole::Editor.is_owner());
    }

    #[test]
    fn state_enums_keep_their_variant_names_on_the_wire() {
        assert_eq!(
            serde_json::to_value(SpaceShareState::Archived).unwrap(),
            json!("Archived")
        );
        assert_eq!(SpaceShareState::default(), SpaceShareState::Personal);
        assert!(SpaceShareState::Personal.is_personal());
        assert!(!SpaceShareState::Shared.is_personal());
        assert_eq!(
            serde_json::to_value(ArchiveReason::Removed).unwrap(),
            json!("Removed")
        );
        assert_eq!(
            serde_json::to_value(InviteKind::Link).unwrap(),
            json!("Link")
        );
        for (status, name) in [
            (InviteStatus::Pending, "Pending"),
            (InviteStatus::Open, "Open"),
            (InviteStatus::Used, "Used"),
            (InviteStatus::Expired, "Expired"),
            (InviteStatus::Revoked, "Revoked"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), json!(name));
            assert_eq!(
                serde_json::from_value::<InviteStatus>(json!(name)).unwrap(),
                status
            );
        }
        for (state, name) in [
            (InviteSessionState::Idle, "Idle"),
            (InviteSessionState::Starting, "Starting"),
            (InviteSessionState::Waiting, "Waiting"),
            (InviteSessionState::Confirming, "Confirming"),
            (InviteSessionState::PeerConfirming, "PeerConfirming"),
            (InviteSessionState::Previewing, "Previewing"),
            (InviteSessionState::Transferring, "Transferring"),
            (InviteSessionState::Joining, "Joining"),
            (InviteSessionState::Done, "Done"),
            (InviteSessionState::Failed, "Failed"),
            (InviteSessionState::Cancelled, "Cancelled"),
        ] {
            assert_eq!(serde_json::to_value(state).unwrap(), json!(name));
            assert_eq!(
                serde_json::from_value::<InviteSessionState>(json!(name)).unwrap(),
                state
            );
        }
        assert_eq!(
            serde_json::to_value((InviteFlow::Code, InviteSide::Invitee)).unwrap(),
            json!(["Code", "Invitee"])
        );
    }

    #[test]
    fn settings_default_to_thirty_days_and_no_editor_links() {
        let settings = SpaceSettings::default();
        assert_eq!(settings.trash_retention_days, 30);
        assert!(!settings.editors_may_link);
        let json = serde_json::to_value(settings).unwrap();
        assert_eq!(
            json,
            json!({ "trashRetentionDays": 30, "editorsMayLink": false })
        );
        assert_eq!(
            serde_json::from_value::<SpaceSettings>(json).unwrap(),
            settings
        );
    }

    #[test]
    fn the_share_plan_is_camel_case() {
        let plan = SpaceSharePlan {
            space_id: SpaceId::from_bytes([1; 16]),
            page_count: 12,
            trashed_page_count: 2,
            attachment_count: 3,
            attachment_bytes: 4_500_000_000,
            history_bytes: 90_000,
            server_configured: true,
            has_account: true,
            has_profile_name: false,
            already_shared: false,
        };
        let json = serde_json::to_value(&plan).unwrap();
        for key in [
            "spaceId",
            "pageCount",
            "trashedPageCount",
            "attachmentCount",
            "attachmentBytes",
            "historyBytes",
            "serverConfigured",
            "hasAccount",
            "hasProfileName",
            "alreadyShared",
        ] {
            assert!(json.get(key).is_some(), "{key}");
        }
        assert_eq!(json["attachmentBytes"], 4_500_000_000_u64);
        assert_eq!(
            serde_json::from_value::<SpaceSharePlan>(json).unwrap(),
            plan
        );
    }

    #[test]
    fn members_round_trip_with_their_optional_fields_left_out() {
        let me = MemberInfo {
            account_id: account(1),
            name: "Alice".into(),
            role: MemberRole::Owner,
            joined_at: 1_791_383_400_000,
            is_me: true,
            verified: false,
            safety_number: None,
            device_count: 2,
            via_kit_at: None,
            refused_changes: 0,
            name_shared: false,
        };
        let json = serde_json::to_value(&me).unwrap();
        assert!(json.get("safetyNumber").is_none() && json.get("viaKitAt").is_none());
        assert_eq!(json["role"], "owner");
        assert_eq!(json["isMe"], true);
        assert_eq!(json["deviceCount"], 2);
        assert_eq!(serde_json::from_value::<MemberInfo>(json).unwrap(), me);

        let bob = MemberInfo {
            account_id: account(2),
            name: "Bob".into(),
            role: MemberRole::Editor,
            joined_at: 1_791_383_500_000,
            is_me: false,
            verified: true,
            safety_number: Some("12345 67890 12345 67890 12345 67890".into()),
            device_count: 1,
            via_kit_at: Some(1_791_384_000_000),
            refused_changes: 3,
            name_shared: true,
        };
        let json = serde_json::to_value(&bob).unwrap();
        assert_eq!(json["safetyNumber"], "12345 67890 12345 67890 12345 67890");
        assert_eq!(json["viaKitAt"], 1_791_384_000_000_i64);
        assert_eq!(json["refusedChanges"], 3);
        assert_eq!(json["nameShared"], true);
        assert_eq!(serde_json::from_value::<MemberInfo>(json).unwrap(), bob);
    }

    #[test]
    fn the_members_info_of_a_space_round_trips() {
        let info = MembersInfo {
            space_id: SpaceId::from_bytes([3; 16]),
            feed_id: FeedId::from_bytes([4; 16]),
            my_role: MemberRole::Owner,
            epoch: 2,
            pending_rekey: true,
            closed_at: None,
            members: Vec::new(),
            former_members: vec![FormerMemberInfo {
                account_id: account(6),
                name: "Dan".into(),
            }],
            invites: vec![InviteInfo {
                invite_id: InviteId::from_bytes([5; 16]),
                kind: InviteKind::Code,
                role: MemberRole::Commenter,
                created_by: account(1),
                created_at: 10,
                expires_at: 20,
                bound_account: Some(account(2)),
                status: InviteStatus::Open,
                used_by: None,
            }],
            settings: SpaceSettings::default(),
            disputes: vec![DisputeInfo {
                page_id: None,
                account_id: account(2),
                at: 30,
            }],
            refused_total: 1,
            server_flagged: false,
        };
        let json = serde_json::to_value(&info).unwrap();
        assert!(json.get("closedAt").is_none());
        for key in [
            "spaceId",
            "feedId",
            "myRole",
            "epoch",
            "pendingRekey",
            "members",
            "formerMembers",
            "invites",
            "settings",
            "disputes",
            "refusedTotal",
            "serverFlagged",
        ] {
            assert!(json.get(key).is_some(), "{key}");
        }
        assert_eq!(
            json["invites"][0]["inviteId"],
            InviteId::from_bytes([5; 16]).to_string()
        );
        assert_eq!(json["invites"][0]["boundAccount"], account(2).to_string());
        assert!(json["invites"][0].get("usedBy").is_none());
        assert!(json["disputes"][0].get("pageId").is_none());
        assert_eq!(json["settings"]["trashRetentionDays"], 30);
        assert_eq!(json["formerMembers"][0]["name"], "Dan");
        assert_eq!(
            json["formerMembers"][0]["accountId"],
            account(6).to_string()
        );
        // A message without the field (an older core) reads as no former members.
        let mut without = json.clone();
        without.as_object_mut().unwrap().remove("formerMembers");
        assert!(
            serde_json::from_value::<MembersInfo>(without)
                .unwrap()
                .former_members
                .is_empty()
        );
        assert_eq!(serde_json::from_value::<MembersInfo>(json).unwrap(), info);

        let closed = MembersInfo {
            closed_at: Some(99),
            ..info
        };
        let json = serde_json::to_value(&closed).unwrap();
        assert_eq!(json["closedAt"], 99);
        assert_eq!(serde_json::from_value::<MembersInfo>(json).unwrap(), closed);
    }

    #[test]
    fn an_invite_link_and_a_session_have_their_shapes() {
        let link = InviteLinkInfo {
            invite_id: InviteId::from_bytes([6; 16]),
            link: "atomik-kb://invite?server=https%3A%2F%2Fkb.example#ABCD".into(),
            web_link: "https://kb.example/join#ABCD".into(),
            role: MemberRole::Viewer,
            expires_at: 1_792_000_000_000,
        };
        let json = serde_json::to_value(&link).unwrap();
        assert_eq!(json["webLink"], "https://kb.example/join#ABCD");
        assert_eq!(json["expiresAt"], 1_792_000_000_000_i64);
        assert_eq!(
            serde_json::from_value::<InviteLinkInfo>(json).unwrap(),
            link
        );

        let idle = InviteSessionInfo {
            flow: InviteFlow::Code,
            side: InviteSide::Inviter,
            state: InviteSessionState::Waiting,
            code: Some("ABCD EFGH JKMN".into()),
            sas: None,
            peer_name: None,
            peer_safety_number: None,
            space_name: None,
            inviter_name: None,
            role: Some(MemberRole::Editor),
            verified: false,
            server_url: Some("https://kb.example".into()),
            error: None,
            expires_at: Some(1_791_384_000_000),
        };
        let json = serde_json::to_value(&idle).unwrap();
        assert_eq!(
            json,
            json!({
                "flow": "Code",
                "side": "Inviter",
                "state": "Waiting",
                "code": "ABCD EFGH JKMN",
                "role": "editor",
                "verified": false,
                "serverUrl": "https://kb.example",
                "expiresAt": 1_791_384_000_000_i64
            })
        );
        assert_eq!(
            serde_json::from_value::<InviteSessionInfo>(json).unwrap(),
            idle
        );

        // Only the four required keys: everything else is optional on the wire.
        let minimal: InviteSessionInfo = serde_json::from_value(json!({
            "flow": "Link", "side": "Invitee", "state": "Previewing", "verified": false
        }))
        .unwrap();
        assert_eq!(minimal.flow, InviteFlow::Link);
        assert!(minimal.code.is_none() && minimal.role.is_none() && minimal.error.is_none());
    }

    #[test]
    fn profiles_people_and_feed_infos_round_trip() {
        let empty = ProfileInfo { display_name: None };
        assert_eq!(serde_json::to_value(&empty).unwrap(), json!({}));
        let named: ProfileInfo = serde_json::from_value(json!({ "displayName": "Alice" })).unwrap();
        assert_eq!(named.display_name.as_deref(), Some("Alice"));

        let person = PersonInfo {
            account_id: account(7),
            name: "Bob".into(),
            verified: true,
            safety_number: None,
            is_me: false,
        };
        let json = serde_json::to_value(&person).unwrap();
        assert!(json.get("safetyNumber").is_none());
        assert_eq!(json["isMe"], false);
        assert_eq!(serde_json::from_value::<PersonInfo>(json).unwrap(), person);

        let feed = FeedSyncInfo {
            feed_id: FeedId::from_bytes([8; 16]),
            space_id: None,
            pending_entries: 4,
            refused: 1,
            forked: false,
            closed: false,
            server_flagged: true,
        };
        let json = serde_json::to_value(&feed).unwrap();
        assert!(json.get("spaceId").is_none());
        assert_eq!(json["pendingEntries"], 4);
        assert_eq!(json["serverFlagged"], true);
        assert_eq!(serde_json::from_value::<FeedSyncInfo>(json).unwrap(), feed);
    }

    #[test]
    fn the_presence_constants_have_the_documented_values() {
        assert_eq!(INLINE_ENTRY_MAX_BYTES, 65_536);
        assert_eq!(PRESENCE_MAX_FRAME_BYTES, 4096);
        assert_eq!(PRESENCE_FRAMES_PER_SECOND, 10);
        assert_eq!(PRESENCE_UPDATES_PER_SECOND, 10);
        assert_eq!(PRESENCE_TIMEOUT_SECONDS, 30);
        assert_eq!(PUSH_DEBOUNCE_PRESENCE_MS, 150);
        assert_eq!(PRESENCE_LABEL, "presence");
        // The presence debounce is shorter than the ordinary one, and an update fits a frame.
        const { assert!(PUSH_DEBOUNCE_PRESENCE_MS < crate::sync::PUSH_DEBOUNCE_MS) };
        const { assert!(PRESENCE_UPDATE_MAX_BYTES < PRESENCE_MAX_FRAME_BYTES) };
        // An inline entry is far below the server's largest object.
        const { assert!(INLINE_ENTRY_MAX_BYTES as u64 <= crate::sync::MAX_WIRE_BYTES) };
    }

    #[test]
    fn presence_states_keep_their_names_and_carry_their_bytes() {
        for (state, name, byte) in [
            (PresenceState::Viewing, "Viewing", 1u8),
            (PresenceState::Editing, "Editing", 2),
            (PresenceState::Idle, "Idle", 3),
        ] {
            assert_eq!(serde_json::to_value(state).unwrap(), json!(name));
            assert_eq!(
                serde_json::from_value::<PresenceState>(json!(name)).unwrap(),
                state
            );
            assert_eq!(state.to_byte(), byte);
            assert_eq!(PresenceState::from_byte(byte), Some(state));
        }
        for bad in [0u8, 4, 0x80, 0xff] {
            assert_eq!(PresenceState::from_byte(bad), None);
        }
        assert!(serde_json::from_value::<PresenceState>(json!("viewing")).is_err());
    }

    #[test]
    fn a_presence_info_and_its_message_are_camel_case() {
        let page = PageId::from_bytes([3; 16]);
        let info = PresenceInfo {
            account_id: account(1),
            device_id: DeviceId::from_bytes([2; 16]),
            name: "Bob".into(),
            color: 4,
            page_id: Some(page),
            state: PresenceState::Editing,
            updated_at: 1_791_383_400_000,
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(
            json,
            json!({
                "accountId": account(1).to_string(),
                "deviceId": DeviceId::from_bytes([2; 16]).to_string(),
                "name": "Bob",
                "color": 4,
                "pageId": page.to_string(),
                "state": "Editing",
                "updatedAt": 1_791_383_400_000_i64
            })
        );
        assert_eq!(serde_json::from_value::<PresenceInfo>(json).unwrap(), info);

        // No page: the key is left out, and reads back as `None`.
        let idle = PresenceInfo {
            page_id: None,
            state: PresenceState::Idle,
            ..info.clone()
        };
        let json = serde_json::to_value(&idle).unwrap();
        assert!(json.get("pageId").is_none());
        assert_eq!(serde_json::from_value::<PresenceInfo>(json).unwrap(), idle);

        // The message carries the update as an array of numbers.
        let message = PresenceMessage {
            info,
            update: vec![0, 1, 255],
        };
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["update"], json!([0, 1, 255]));
        assert_eq!(json["info"]["color"], 4);
        assert_eq!(
            serde_json::from_value::<PresenceMessage>(json).unwrap(),
            message
        );
        // A colour index is one byte.
        assert!(
            serde_json::from_value::<PresenceInfo>(json!({
                "accountId": account(1).to_string(),
                "deviceId": DeviceId::from_bytes([2; 16]).to_string(),
                "name": "Bob", "color": 256, "state": "Viewing", "updatedAt": 0
            }))
            .is_err()
        );
    }
    #[test]
    fn the_view_link_constants_have_the_documented_values() {
        assert_eq!(LINK_REFRESH_DEBOUNCE_MS, 60_000);
        assert_eq!(LINK_POLL_SECONDS, 60);
        assert_eq!(LINK_GETS_PER_MINUTE_PER_IP, 120);
        assert_eq!(LINK_ID_LABEL, "link-id");
        assert_eq!(PUBLICATION_LABEL, "publication");
        assert_eq!(VIEWER_PATH, "/view");
        assert_eq!((LINK_EXPIRY_MAX_DAYS, LINK_MAX_CHUNKS), (365, 1024));
        // The policy of ADR-0051, word for word: the viewer's scripts and wasm, its own origin for
        // requests, its own origin and the blobs it decrypts for images, nothing else.
        assert_eq!(
            VIEWER_CSP,
            "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; img-src 'self' blob:"
        );
        assert!(!VIEWER_CSP.contains("http"), "no remote origin is named");
        // The poll is not faster than the refresh it follows can matter, and a page's declared
        // chunks fit one publication upload.
        const { assert!(LINK_POLL_SECONDS * 1000 <= LINK_REFRESH_DEBOUNCE_MS) };
        const { assert!((LINK_MAX_CHUNKS as u64) * 32 < crate::sync::MAX_WIRE_BYTES) };
    }

    #[test]
    fn a_view_link_info_is_camel_case_and_leaves_absent_times_out() {
        let link = LinkId::from_bytes([0xcd; 16]);
        let page = PageId::from_bytes([3; 16]);
        let info = ViewLinkInfo {
            link_id: link,
            page_id: page,
            url: "https://kb.example/view#KEY".into(),
            created_by: account(1),
            created_by_name: "Alice".into(),
            created_at: 1_791_383_400_000,
            expires_at: Some(1_791_988_200_000),
            last_published_at: Some(1_791_383_460_000),
            status: ViewLinkStatus::Live,
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(
            json,
            json!({
                "linkId": link.to_string(),
                "pageId": page.to_string(),
                "url": "https://kb.example/view#KEY",
                "createdBy": account(1).to_string(),
                "createdByName": "Alice",
                "createdAt": 1_791_383_400_000_i64,
                "expiresAt": 1_791_988_200_000_i64,
                "lastPublishedAt": 1_791_383_460_000_i64,
                "status": "Live"
            })
        );
        assert_eq!(serde_json::from_value::<ViewLinkInfo>(json).unwrap(), info);

        // A link that never expires and was never published: both keys are left out.
        let fresh = ViewLinkInfo {
            expires_at: None,
            last_published_at: None,
            status: ViewLinkStatus::Publishing,
            ..info
        };
        let json = serde_json::to_value(&fresh).unwrap();
        assert!(json.get("expiresAt").is_none() && json.get("lastPublishedAt").is_none());
        assert_eq!(json["status"], "Publishing");
        assert_eq!(serde_json::from_value::<ViewLinkInfo>(json).unwrap(), fresh);
    }

    #[test]
    fn view_link_states_keep_their_names() {
        for (status, name) in [
            (ViewLinkStatus::Publishing, "Publishing"),
            (ViewLinkStatus::Live, "Live"),
            (ViewLinkStatus::Expired, "Expired"),
            (ViewLinkStatus::Revoked, "Revoked"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), json!(name));
            assert_eq!(
                serde_json::from_value::<ViewLinkStatus>(json!(name)).unwrap(),
                status
            );
        }
        assert!(serde_json::from_value::<ViewLinkStatus>(json!("live")).is_err());
    }
}
