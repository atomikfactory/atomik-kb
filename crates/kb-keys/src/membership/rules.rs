//! The stable names of the rules the fold applies ([`FoldRule::Rule`](super::FoldRule::Rule)).
//!
//! Each name is a short identifier without any data, so errors never carry an entry's content.
//! The categories that have their own [`FoldRule`](super::FoldRule) variant (encoding, chain,
//! signature, unknown log, completeness, heads) are not repeated here. Tests of other crates and
//! the server match on these constants, never on their text.

/// An entry follows the `Close`.
pub const CLOSED: &str = "space_closed";
/// The signer is not a member at this point of the log (never joined, removed, or left).
pub const SIGNER_NOT_MEMBER: &str = "signer_not_member";
/// An Owner kind (`Invite`, `InviteRevoke`, `RoleSet`, `Remove`, `Settings`, `Close`) by a member
/// who is not an Owner.
pub const SIGNER_NOT_OWNER: &str = "signer_not_owner";
/// `meta_epoch` is above the epoch in force after the entry, or not the `Create`'s own epoch.
pub const META_EPOCH: &str = "meta_epoch";

/// An `Invite` that grants the Owner role.
pub const INVITE_GRANTS_OWNER: &str = "invite_grants_owner";
/// An `Invite` that expires more than `INVITE_MAX_DAYS` after its `at`.
pub const INVITE_EXPIRY_TOO_FAR: &str = "invite_expiry_too_far";
/// An `Invite` whose id the log holds already (in any state).
pub const INVITE_ID_EXISTS: &str = "invite_id_exists";
/// An `Invite` when `MAX_OPEN_INVITES` invites are open (open and not yet expired at its `at`).
pub const TOO_MANY_OPEN_INVITES: &str = "too_many_open_invites";
/// An `InviteRevoke` or a `Join` that names an invite the log does not hold.
pub const INVITE_UNKNOWN: &str = "invite_unknown";
/// An `InviteRevoke` or a `Join` that names an invite a `Join` consumed (a replay).
pub const INVITE_USED: &str = "invite_used";
/// An `InviteRevoke` or a `Join` that names a revoked invite (revoked by an Owner, or void because
/// its creator is no longer an Owner).
pub const INVITE_REVOKED: &str = "invite_revoked";
/// A `Join` by an account other than the one the invite is bound to.
pub const INVITE_BOUND_TO_OTHER_ACCOUNT: &str = "invite_bound_to_other_account";
/// A `Join` whose account is not the account that signs it.
pub const JOIN_ACCOUNT_MISMATCH: &str = "join_account_mismatch";
/// A `Join` whose proof is not a signature of the invite key over the Join's fields.
pub const JOIN_PROOF_INVALID: &str = "join_proof_invalid";
/// A `Join` by an account that is a member already.
pub const ALREADY_MEMBER: &str = "already_member";
/// A `Join` into a space that holds `MAX_MEMBERS_PER_FEED` members.
pub const SPACE_FULL: &str = "space_full";

/// A `RoleSet`, `Remove` or `Grant` whose target is not a member.
pub const TARGET_NOT_MEMBER: &str = "target_not_member";
/// A demotion or a `Leave` that would leave the space without an Owner.
pub const LAST_OWNER: &str = "last_owner";
/// A `Remove` whose target is its signer.
pub const REMOVE_SELF: &str = "remove_self";

/// A `Grant` for another epoch than the current one.
pub const GRANT_NOT_CURRENT_EPOCH: &str = "grant_not_current_epoch";
/// A second `Grant` for one member and epoch.
pub const GRANT_EXISTS: &str = "grant_exists";
/// A `Grant` without boxes, or with two boxes for one device.
pub const GRANT_BOXES_INVALID: &str = "grant_boxes_invalid";
