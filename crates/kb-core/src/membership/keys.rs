//! The keys of a shared space on this device: the space keys of a feed's epochs (stored as
//! `Space(epoch)` records of the personal keyring, ADR-0044), the backward key chain, and the stash
//! that holds a key until the space id is known (the link flow's invitee learns it from the first
//! key record of the feed).

use kb_crypto::{Label, Secret32, WrappedKey};
use kb_keys::KeyringItem;
use kb_keys::keyring as kr;
use kb_keys::space_keys::{SpaceKey, unwrap_prev};
use kb_storage::{FeedQueries, SyncQueries};
use kb_types::{ApiError, FeedId, SpaceId};

use super::feed_key;
use crate::security::{decode_hex, hex};
use crate::{Core, map_storage_error};

/// The key-encryption key of the stash: `HKDF(ARK, "space-kek", info = feed id)`.
fn stash_kek(ark: &kb_keys::Ark, feed: FeedId) -> Secret32 {
    kb_crypto::derive_key(ark.secret(), Label::SPACE_KEK, feed.as_bytes())
}

/// The AAD subject of a stashed key: `feed || epoch`.
fn stash_subject(feed: FeedId, epoch: u32) -> Vec<u8> {
    let mut subject = feed.as_bytes().to_vec();
    subject.extend_from_slice(&epoch.to_le_bytes());
    subject
}

impl Core {
    /// The space key of `epoch` of a shared feed: the `Space(epoch)` record of the personal keyring
    /// when the feed's space is known, the stash otherwise; `None` when this device holds none.
    pub(crate) fn feed_sk(
        &mut self,
        feed: FeedId,
        epoch: u32,
    ) -> Result<Option<SpaceKey>, ApiError> {
        if let Some(space_id) = self.feed_space(feed)? {
            return Ok(self
                .space_key_at(space_id, epoch)?
                .map(SpaceKey::from_secret));
        }
        self.stash_get(feed, epoch)
    }

    /// Stores a space key this device learned (opened from a box, a bundle, or down the key chain):
    /// as a `Space(epoch)` record of the personal keyring wrapped under the ARK (the personal feed
    /// carries it to the account's other devices), or in the stash while the feed has no space yet.
    /// A key that is stored already stays.
    pub(crate) fn feed_store_sk(
        &mut self,
        feed: FeedId,
        epoch: u32,
        key: &SpaceKey,
    ) -> Result<(), ApiError> {
        let Some(space_id) = self.feed_space(feed)? else {
            return self.stash_put(feed, epoch, key);
        };
        if self.space_key_at(space_id, epoch)?.is_some() {
            return Ok(());
        }
        let now = self.clock.now_ms();
        let record = kr::wrap_space_key(
            key.secret(),
            &self.ark,
            space_id,
            epoch,
            self.security.ark_generation,
            &self.security.device_keys,
            self.device.device_id,
            now,
            self.rng.as_ref(),
        );
        self.put_keyring_item(KeyringItem::Record(record))?;
        self.security
            .space_keys
            .insert((space_id, epoch), key.secret().clone());
        Ok(())
    }

    /// Opens the older epochs' keys down the backward chain: the key of `epoch` unwraps the key of
    /// `epoch - 1` (`prev_wrapped` of the entry that began `epoch`), checked against that epoch's
    /// commitment, so a member who joined later reads the whole history.
    pub(crate) fn feed_derive_older_keys(&mut self, feed: FeedId) -> Result<(), ApiError> {
        let Some(meta) = self.shared.metas.get(&feed).cloned() else {
            return Ok(());
        };
        let top = self
            .shared
            .timelines
            .get(&feed)
            .map_or(0, |t| t.state().epoch);
        for epoch in (1..=top).rev() {
            let Some(newer) = self.feed_sk(feed, epoch)? else {
                continue;
            };
            let Some(wrapped) = meta.prev_wrapped.get(&epoch) else {
                continue;
            };
            let older = epoch - 1;
            if self.feed_sk(feed, older)?.is_some() {
                continue;
            }
            let Ok(key) = unwrap_prev(&newer, wrapped, feed, older) else {
                tracing::warn!(feed = %feed.short_hex(), "a previous space key does not unwrap");
                continue;
            };
            if let Some(commitment) = meta.commitments.get(&older)
                && key.commitment(feed, older) != *commitment
            {
                tracing::warn!(feed = %feed.short_hex(), "a previous space key does not match its commitment");
                continue;
            }
            self.feed_store_sk(feed, older, &key)?;
        }
        Ok(())
    }

    // ----- the stash ------------------------------------------------------------------------

    fn stash_put(&mut self, feed: FeedId, epoch: u32, key: &SpaceKey) -> Result<(), ApiError> {
        if self.stash_get(feed, epoch)?.is_some() {
            return Ok(());
        }
        let wrapped = kb_crypto::wrap_key(
            key.secret(),
            &stash_kek(&self.ark, feed),
            &stash_subject(feed, epoch),
            self.rng.as_ref(),
        );
        self.fcfg_set(feed, &format!("sk.{epoch}"), &hex(&wrapped.to_bytes()))
    }

    fn stash_get(&mut self, feed: FeedId, epoch: u32) -> Result<Option<SpaceKey>, ApiError> {
        let Some(text) = self.fcfg(feed, &format!("sk.{epoch}"))? else {
            return Ok(None);
        };
        let Some(bytes) = decode_hex(&text) else {
            return Ok(None);
        };
        let Ok(wrapped) = WrappedKey::from_bytes(&bytes) else {
            return Ok(None);
        };
        Ok(kb_crypto::unwrap_key(
            &wrapped,
            &stash_kek(&self.ark, feed),
            &stash_subject(feed, epoch),
        )
        .ok()
        .map(SpaceKey::from_secret))
    }

    /// The epochs whose key sits in the stash of a feed.
    fn stash_epochs(&self, feed: FeedId) -> Result<Vec<u32>, ApiError> {
        let prefix = feed_key(feed, "sk.");
        Ok(self
            .db
            .sync_config_all()
            .map_err(map_storage_error)?
            .into_iter()
            .filter_map(|(key, _)| key.strip_prefix(&prefix)?.parse::<u32>().ok())
            .collect())
    }

    /// Binds a shared feed to its space once the space id is known (the first key record of the
    /// feed names it): stashed keys become `Space` records. Refused (`false`) when this device
    /// already has that space or keys for it: a hostile member must not point a feed at a space
    /// the device has.
    pub(crate) fn feed_bind_space(
        &mut self,
        feed: FeedId,
        space_id: SpaceId,
    ) -> Result<bool, ApiError> {
        let Some(mut row) = self.db.sync_feed_get(feed).map_err(map_storage_error)? else {
            return Ok(false);
        };
        if let Some(bound) = row.space_id {
            return Ok(bound == space_id);
        }
        if self.docs.structures.contains_key(&space_id)
            || !self.security.keyring.space_records(space_id).is_empty()
            || self.shared_feed_of(space_id)?.is_some()
        {
            return Ok(false);
        }
        let epochs = self.stash_epochs(feed)?;
        let mut keys = Vec::new();
        for epoch in &epochs {
            if let Some(key) = self.stash_get(feed, *epoch)? {
                keys.push((*epoch, key));
            }
        }
        row.space_id = Some(space_id);
        self.db.sync_feed_put(&row).map_err(map_storage_error)?;
        for (epoch, key) in keys {
            self.feed_store_sk(feed, epoch, &key)?;
            self.fcfg_del(feed, &format!("sk.{epoch}"))?;
        }
        self.ensure_account()?;
        if let Some(account) = self.docs.account.as_ref() {
            account
                .model
                .set_shared_space(space_id, feed)
                .map_err(crate::documents::map_model_error)?;
        }
        self.persist_account()?;
        self.feed_derive_older_keys(feed)?;
        Ok(true)
    }
}
