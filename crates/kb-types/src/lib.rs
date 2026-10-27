//! Shared types for atomik-kb.
//!
//! This crate holds the three things every other crate agrees on:
//!
//! - typed 128-bit identifiers ([`WorkspaceId`], [`PageId`], ...);
//! - the core API contract ([`Request`], [`Response`], [`Event`], [`ApiError`])
//!   that crosses the boundary to every shell (ADR-0015);
//! - platform traits the core depends on but never implements ([`Clock`], [`Rng`]).
//!
//! It contains no business logic and must compile for `wasm32-unknown-unknown`.

#![forbid(unsafe_code)]

pub mod ai;
mod api;
pub mod attachments;
pub mod comments;
pub mod devices;
mod ids;
pub mod interop;
mod platform;
pub mod properties;
pub mod sharing;
pub mod sync;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use api::*;
pub use comments::{
    CommentAnchor, CommentBodyItem, CommentInfo, NotificationInfo, NotificationKind, ThreadInfo,
};
pub use ids::*;
pub use interop::{
    BackupInterval, BackupRunStatus, BackupSchedule, CsvColumn, ExportBatch, ExportContent,
    ExportEntry, ExportFormat, ExportPlan, ExportWarning, ExportWarningCode, ImportPlanInfo,
    ImportProgress, ImportRemoteFile, ImportReport, ImportSpaceInfo, ImportWarning,
    ImportWarningCode,
};
pub use platform::*;
pub use properties::{
    ChecklistItem, DateValue, FormulaBase, FormulaResult, FormulaType, OptionColor, PageProperties,
    PlaceValue, PropertyConfig, PropertyDef, PropertyFilter, PropertyKind, PropertyOption,
    PropertySchema, PropertyValue, RollupFunction, RollupResult, RollupTarget, RollupValue,
    StatusGroup,
};
pub use sharing::{
    ArchiveReason, DisputeInfo, FeedSyncInfo, FormerMemberInfo, InviteFlow, InviteInfo, InviteKind,
    InviteLinkInfo, InviteSessionInfo, InviteSessionState, InviteSide, InviteStatus, MemberInfo,
    MemberRole, MembersInfo, PersonInfo, PresenceInfo, PresenceMessage, PresenceState, ProfileInfo,
    SpaceSettings, SpaceSharePlan, SpaceShareState, ViewLinkInfo, ViewLinkStatus,
};

/// Version of the request/response/event contract. Additive changes keep the number;
/// a breaking change bumps it and the shells refuse to talk to a core they do not know.
pub const API_VERSION: u32 = 1;

/// Version of the core, reported in [`AppStatus`].
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// `serde_json::Value` under a short name; the only dynamic payload type in the API.
pub type Json = serde_json::Value;
