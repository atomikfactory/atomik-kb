//! Comments and notifications (Milestone 13 slice 2, ADR-0048 and ADR-0049,
//! `docs/security/sharing.md` section 8, `docs/development/milestone-13.md` section 2.1): the
//! constants of the feature and the plain data of its API (anchors, comment bodies, comments,
//! threads, notifications). Pure data; the signed records, their fold and the role rules live in
//! `kb-model` and `kb-core`, the stream's storage in `kb-storage`.
//!
//! The constants are not glob re-exported: refer to them as `kb_types::comments::...`. The types are
//! re-exported at the crate root like every other API type.
//!
//! JSON shapes (camelCase fields, as everywhere in the API; optional fields are left out when absent
//! and optional in TypeScript):
//!
//! ```text
//! CommentAnchor    {"blockId": "<block id>", "start": "<base64 cursor>", "end": "<base64 cursor>", "quote": "the words"}
//! CommentBodyItem  {"type": "text", "text": "Looks good, "}
//!                  {"type": "mention", "accountId": "<account id>"}
//!                  {"type": "page", "id": "<page id>"}
//! ThreadInfo       {"threadId": ..., "pageId": ..., "anchor": {...}, "resolved": false, "comments": [CommentInfo]}
//! NotificationInfo {"id": 7, "kind": "ThreadReply", "spaceId": ..., "pageId": ..., "actor": ..., "actorName": "Bob", "at": 1760000000000, "read": false}
//! ```

use serde::{Deserialize, Serialize};

use crate::ids::{AccountId, BlockId, CommentId, PageId, SpaceId, ThreadId};

// ----- constants ----------------------------------------------------------------------------

/// Domain of a comment stream's document id (ADR-0048): `doc_id = BLAKE3(COMMENT_STREAM_DOMAIN ||
/// page_id)[..16]`, with the page id as its 16 bytes. The id is deterministic, so two Commenters
/// never make two streams for one page.
pub const COMMENT_STREAM_DOMAIN: &str = "atomik-kb/v1/comments";
/// Context a comment record's signature is made under (`kb_crypto::sign`, ADR-0048).
pub const COMMENT_RECORD_CONTEXT: &str = "comment-record";
/// The most a comment's body may weigh, serialised as JSON (`Vec<CommentBodyItem>`): 10 KB.
pub const COMMENT_BODY_MAX_BYTES: usize = 10 * 1024;
/// The longest quote of a [`CommentAnchor`], in characters.
pub const COMMENT_QUOTE_MAX_CHARS: usize = 200;
/// How many notifications `NotificationList` answers when the request gives no limit.
pub const NOTIFICATIONS_DEFAULT_LIMIT: u32 = 50;
/// The most notifications one `NotificationList` answers, whatever the limit asked for.
pub const NOTIFICATIONS_MAX_LIMIT: u32 = 200;
/// The most rows the device's `notifications` table keeps: the oldest go first when a new one is
/// added past it (read ones before unread ones).
pub const NOTIFICATIONS_MAX_ROWS: usize = 1000;

// ----- comments ----------------------------------------------------------------------------

/// Where a thread's first comment points (ADR-0048): a block and optionally a range of its text,
/// kept as Loro cursors so the range follows later edits. Comments live outside the page document,
/// so an anchor is a hint: one the editor cannot resolve (the block or the text is gone) shows as a
/// page-level thread with its quote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct CommentAnchor {
    /// The block the comment is about; absent for a comment on the page as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub block_id: Option<BlockId>,
    /// The start of the range, as base64 of the Loro cursor bytes (`InvalidInput` for `anchor` when
    /// it does not decode); absent for a block-level anchor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub start: Option<String>,
    /// The end of the range, as base64 of the Loro cursor bytes; absent for a block-level anchor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub end: Option<String>,
    /// The words the range covered when the comment was posted, at most
    /// [`COMMENT_QUOTE_MAX_CHARS`] characters (`InvalidInput` for `anchor` beyond that).
    pub quote: String,
}

/// One piece of a comment's body (ADR-0048). A body is a `Vec<CommentBodyItem>`, at most
/// [`COMMENT_BODY_MAX_BYTES`] serialised, and not empty (`InvalidInput` for `body`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum CommentBodyItem {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// A mention of a member (`@name`): notifies them (`CommentMention`). It shows the member's
    /// current name, or "Former member" when they left.
    Mention {
        /// The mentioned account.
        account_id: AccountId,
    },
    /// A reference to a page, shown with the page's title.
    Page {
        /// The page.
        id: PageId,
    },
}

/// One comment as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct CommentInfo {
    /// The comment.
    pub comment_id: CommentId,
    /// Its thread (`threadId == commentId` for the root).
    pub thread_id: ThreadId,
    /// The page.
    pub page_id: PageId,
    /// The author's account.
    pub author: AccountId,
    /// The author's name as the space's membership says; a removed author's last known name (the
    /// UI shows "Former member" instead when [`Self::author_former`] is set).
    pub author_name: String,
    /// The author is no longer a member of the space (removed or left): the UI shows "Former member".
    /// Their comments stay (ADR-0048).
    pub author_former: bool,
    /// The body, as the latest valid `Edit` (or the `Post`) holds it; empty for a deleted comment.
    pub body: Vec<CommentBodyItem>,
    /// When the comment was posted, Unix ms (the author's device clock: a claim, not proof).
    pub created_at: i64,
    /// When the body last changed, Unix ms; absent when it never did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub edited_at: Option<i64>,
    /// The comment was deleted: kept as a placeholder while its thread has replies (`body` is empty).
    pub deleted: bool,
    /// This account wrote it.
    pub is_mine: bool,
}

/// One thread of a page: a root comment and its replies, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ThreadInfo {
    /// The thread (the id of its root comment).
    pub thread_id: ThreadId,
    /// The page.
    pub page_id: PageId,
    /// Where the root comment points; absent for a comment on the page as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub anchor: Option<CommentAnchor>,
    /// The thread was resolved (the last valid `Resolve` says so).
    pub resolved: bool,
    /// Who resolved it, when resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub resolved_by: Option<AccountId>,
    /// The comments in order (the root first, then by `createdAt`, then comment id), deleted ones
    /// that still have replies after them included as placeholders. Empty only in the answer to
    /// `CommentDelete` of the last comment of a thread (the thread is gone from `CommentList`).
    pub comments: Vec<CommentInfo>,
}

// ----- notifications --------------------------------------------------------------------------

/// Why a notification was raised (ADR-0048, ADR-0049). Variant names are kept on the wire, like
/// `PairingState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum NotificationKind {
    /// Somebody replied in a thread this account wrote in.
    ThreadReply,
    /// A comment mentions this account.
    CommentMention,
    /// A page gained a `personMention` of this account.
    PageMention,
    /// This account was added to a Person property of a page.
    PersonAssigned,
}

/// One row of the device's inbox: a remote change that concerns this account (ADR-0048). The core
/// raises it from changes that arrive through sync only, never from this device's own writes, and the
/// server never learns of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct NotificationInfo {
    /// The row id (increasing; `NotificationList` pages by it).
    pub id: i64,
    /// Why.
    pub kind: NotificationKind,
    /// The space.
    pub space_id: SpaceId,
    /// The page the change is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub page_id: Option<PageId>,
    /// The block the change is on, when it has one (a mention in the text; a comment's anchor).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub block_id: Option<BlockId>,
    /// The comment, for the two comment kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub comment_id: Option<CommentId>,
    /// Who did it.
    pub actor: AccountId,
    /// The actor's name as the space says, read when the row is listed (never stored).
    pub actor_name: String,
    /// The page's title, read when the row is listed (never stored); absent when the page is gone.
    /// The OS notification's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub page_title: Option<String>,
    /// When the change was seen on this device, Unix ms.
    pub at: i64,
    /// Marked read (`NotificationsMarkRead`).
    pub read: bool,
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
    fn body_items_are_tagged_by_type_with_camel_case_fields() {
        let body = vec![
            CommentBodyItem::Text {
                text: "Looks good, ".into(),
            },
            CommentBodyItem::Mention {
                account_id: account(1),
            },
            CommentBodyItem::Page {
                id: PageId::from_bytes([2; 16]),
            },
        ];
        let value = serde_json::to_value(&body).unwrap();
        assert_eq!(
            value,
            json!([
                {"type": "text", "text": "Looks good, "},
                {"type": "mention", "accountId": "01010101-0101-0101-0101-010101010101"},
                {"type": "page", "id": "02020202-0202-0202-0202-020202020202"},
            ])
        );
        let back: Vec<CommentBodyItem> = serde_json::from_value(value).unwrap();
        assert_eq!(back, body);
        assert!(
            serde_json::from_value::<CommentBodyItem>(json!({"type": "emoji", "text": "x"}))
                .is_err()
        );
    }

    #[test]
    fn anchors_leave_out_what_they_do_not_have() {
        let block_only = CommentAnchor {
            block_id: Some(BlockId::from_bytes([3; 16])),
            start: None,
            end: None,
            quote: "the quarterly plan".into(),
        };
        assert_eq!(
            serde_json::to_value(&block_only).unwrap(),
            json!({"blockId": "03030303-0303-0303-0303-030303030303", "quote": "the quarterly plan"})
        );
        let page_level: CommentAnchor = serde_json::from_value(json!({"quote": ""})).unwrap();
        assert_eq!(page_level.block_id, None);
        let ranged = CommentAnchor {
            block_id: Some(BlockId::from_bytes([3; 16])),
            start: Some("AQID".into()),
            end: Some("BAUG".into()),
            quote: "plan".into(),
        };
        let value = serde_json::to_value(&ranged).unwrap();
        assert_eq!(value["start"], "AQID");
        assert_eq!(value["end"], "BAUG");
        assert_eq!(
            serde_json::from_value::<CommentAnchor>(value).unwrap(),
            ranged
        );
    }

    #[test]
    fn threads_and_comments_have_their_wire_shapes() {
        let root = CommentId::from_bytes([4; 16]);
        let comment = CommentInfo {
            comment_id: root,
            thread_id: ThreadId::of(root),
            page_id: PageId::from_bytes([5; 16]),
            author: account(6),
            author_name: "Bob".into(),
            author_former: false,
            body: vec![CommentBodyItem::Text { text: "Hi".into() }],
            created_at: 1_760_000_000_000,
            edited_at: None,
            deleted: false,
            is_mine: true,
        };
        let value = serde_json::to_value(&comment).unwrap();
        assert_eq!(
            value,
            json!({
                "commentId": "04040404-0404-0404-0404-040404040404",
                "threadId": "04040404-0404-0404-0404-040404040404",
                "pageId": "05050505-0505-0505-0505-050505050505",
                "author": "06060606-0606-0606-0606-060606060606",
                "authorName": "Bob",
                "authorFormer": false,
                "body": [{"type": "text", "text": "Hi"}],
                "createdAt": 1_760_000_000_000_i64,
                "deleted": false,
                "isMine": true,
            }),
            "editedAt is left out when the body never changed"
        );
        let former = CommentInfo {
            author_former: true,
            edited_at: Some(1_760_000_100_000),
            ..comment.clone()
        };
        let value = serde_json::to_value(&former).unwrap();
        assert_eq!(value["authorFormer"], true);
        assert_eq!(value["editedAt"], 1_760_000_100_000_i64);
        assert_eq!(
            serde_json::from_value::<CommentInfo>(value).unwrap(),
            former
        );
        let thread = ThreadInfo {
            thread_id: ThreadId::of(root),
            page_id: comment.page_id,
            anchor: None,
            resolved: false,
            resolved_by: None,
            comments: vec![comment],
        };
        let value = serde_json::to_value(&thread).unwrap();
        assert_eq!(value["resolved"], false);
        assert!(value.get("anchor").is_none());
        assert!(value.get("resolvedBy").is_none());
        assert_eq!(serde_json::from_value::<ThreadInfo>(value).unwrap(), thread);
    }

    #[test]
    fn notifications_keep_their_kind_names_and_leave_out_absent_fields() {
        let info = NotificationInfo {
            id: 7,
            kind: NotificationKind::PersonAssigned,
            space_id: SpaceId::from_bytes([8; 16]),
            page_id: Some(PageId::from_bytes([9; 16])),
            block_id: None,
            comment_id: None,
            actor: account(10),
            actor_name: "Alice".into(),
            page_title: Some("Plan".into()),
            at: 1_760_000_000_000,
            read: false,
        };
        let value = serde_json::to_value(&info).unwrap();
        assert_eq!(
            value,
            json!({
                "id": 7,
                "kind": "PersonAssigned",
                "spaceId": "08080808-0808-0808-0808-080808080808",
                "pageId": "09090909-0909-0909-0909-090909090909",
                "actor": "0a0a0a0a-0a0a-0a0a-0a0a-0a0a0a0a0a0a",
                "actorName": "Alice",
                "pageTitle": "Plan",
                "at": 1_760_000_000_000_i64,
                "read": false,
            })
        );
        assert_eq!(
            serde_json::from_value::<NotificationInfo>(value).unwrap(),
            info
        );
        for kind in [
            NotificationKind::ThreadReply,
            NotificationKind::CommentMention,
            NotificationKind::PageMention,
        ] {
            let text = serde_json::to_string(&kind).unwrap();
            assert_eq!(text, format!("\"{kind:?}\""));
        }
    }
}
