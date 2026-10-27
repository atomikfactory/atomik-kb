//! Local notifications (M13 slice 2, ADR-0048 and ADR-0049, `docs/development/milestone-13.md`
//! sections 2.1 and 2.2): this device's inbox, the four triggers that fill it and the two requests
//! that read it.
//!
//! A notification is a row of the device's `notifications` table, raised from a **remote** change
//! only (a change that arrived through sync): this device's own writes never notify, and the server
//! never learns of any. The four triggers:
//!
//! | Kind | When |
//! |---|---|
//! | `ThreadReply` | a valid remote `Post` replies in a thread this account wrote in (its root or a reply) |
//! | `CommentMention` | a valid remote `Post` or `Edit` whose body mentions this account (an edit notifies only for a mention it adds; a reply that also mentions is a mention) |
//! | `PageMention` | a remote change to a page document adds a `personMention` of this account (the diff against `person_mentions`) |
//! | `PersonAssigned` | a remote change adds this account to a Person property value of a page |
//!
//! A row names the actor by account id; its name and the page's title are read when the row is listed
//! (`NotificationInfo::actor_name`, `page_title`) so a rename shows and nothing user-written is
//! stored. [`Core::notification_raise`] adds the row and emits `NotificationRaised` (the in-app toast
//! and, in the shell, the OS notification: the page's title and a phrase with the actor's name, never
//! comment text) and `NotificationsChanged`.
//!
//! **Not raised:** by this account's own account on any device (the actor is this account), for a space
//! this account is leaving or has left, and while a feed is read for the first time (a new device
//! must not announce a space's history: `comments/apply.rs`, `mentions.rs`). A notification that
//! arrives while the app is locked is raised when the changes are applied, which only happens while
//! the core is ready.

use kb_storage::{DocQueries, NewNotification, NotificationQueries, NotificationRow};
use kb_types::comments::{NOTIFICATIONS_DEFAULT_LIMIT, NOTIFICATIONS_MAX_LIMIT};
use kb_types::{ApiError, Event, NotificationInfo, Request, Response};

use crate::{Core, map_storage_error};

impl Core {
    /// Routes `NotificationList` and `NotificationsMarkRead`.
    pub(crate) fn dispatch_notifications(
        &mut self,
        request: Request,
    ) -> Result<Response, ApiError> {
        match request {
            Request::NotificationList {
                limit,
                before_id,
                unread_only,
            } => Ok(Response::Notifications(self.notification_list(
                limit,
                before_id,
                unread_only.unwrap_or(false),
            )?)),
            Request::NotificationsMarkRead { ids } => {
                self.notifications_mark_read(ids.as_deref())?;
                Ok(Response::Ok)
            }
            other => Err(ApiError::internal(format!(
                "{} is not a notification request",
                other.kind()
            ))),
        }
    }

    /// The inbox, newest first: at most `limit` rows (default
    /// `NOTIFICATIONS_DEFAULT_LIMIT`, at most `NOTIFICATIONS_MAX_LIMIT`), only those below `before_id`
    /// when given, only unread ones when `unread_only`, with the actor's name and the page's title
    /// read now.
    pub(crate) fn notification_list(
        &mut self,
        limit: Option<u32>,
        before_id: Option<i64>,
        unread_only: bool,
    ) -> Result<Vec<NotificationInfo>, ApiError> {
        self.shared_load()?;
        let limit = limit.map_or(NOTIFICATIONS_DEFAULT_LIMIT, |limit| {
            limit.min(NOTIFICATIONS_MAX_LIMIT)
        });
        let rows = self
            .db
            .notification_list(limit, before_id, unread_only)
            .map_err(map_storage_error)?;
        Ok(rows
            .into_iter()
            .map(|row| self.notification_info(row))
            .collect())
    }

    /// Marks the listed rows read, or every row when `ids` is `None`. Emits `NotificationsChanged`
    /// when a row changed.
    pub(crate) fn notifications_mark_read(&mut self, ids: Option<&[i64]>) -> Result<(), ApiError> {
        let changed = self
            .db
            .notification_mark_read(ids)
            .map_err(map_storage_error)?;
        if changed > 0 {
            self.push_event_once(Event::NotificationsChanged);
        }
        Ok(())
    }

    /// Adds a row for a remote change and emits `NotificationRaised` (with the row as
    /// `NotificationList` would show it) and `NotificationsChanged`. Called by the apply paths only:
    /// a change this device made is never passed here. Nothing is raised for the actor being this
    /// account, and nothing for a space this account is leaving or has left.
    pub(crate) fn notification_raise(&mut self, row: &NewNotification) -> Result<(), ApiError> {
        if self.me().ok() == Some(row.actor) {
            return Ok(());
        }
        if let Some(feed) = self.shared_feed_of(row.space_id)?
            && (self.feed_archived(feed.feed)?.is_some() || self.space_is_leaving(&feed))
        {
            return Ok(());
        }
        self.shared_load()?;
        let id = self.db.notification_add(row).map_err(map_storage_error)?;
        if let Some(stored) = self.db.notification_get(id).map_err(map_storage_error)? {
            let info = self.notification_info(stored);
            self.events.push(Event::NotificationRaised(info));
        }
        self.push_event_once(Event::NotificationsChanged);
        tracing::debug!(kind = ?row.kind, "a notification was raised");
        Ok(())
    }

    /// A stored row as the API shows it: the actor's name and the page's title are read now.
    fn notification_info(&self, row: NotificationRow) -> NotificationInfo {
        let (actor_name, _) = self.person_in_space(row.space_id, row.actor);
        let page_title = row.page_id.and_then(|page| {
            self.db
                .pages_index_get(page)
                .ok()
                .flatten()
                .map(|index| index.title)
        });
        NotificationInfo {
            id: row.id,
            kind: row.kind,
            space_id: row.space_id,
            page_id: row.page_id,
            block_id: row.block_id,
            comment_id: row.comment_id,
            actor: row.actor,
            actor_name,
            page_title,
            at: row.at,
            read: row.read,
        }
    }
}
