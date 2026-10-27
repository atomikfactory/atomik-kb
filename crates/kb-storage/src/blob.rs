//! The blob store: content-addressed encrypted attachment chunks (ADR-0020).
//!
//! A chunk is one ciphertext blob of at most `CHUNK_SIZE + 40` bytes, addressed by the
//! BLAKE3 hash of its ciphertext ([`ChunkId`]). The store is dumb: it never looks inside
//! a chunk, knows nothing about attachments or manifests, and every operation handles one
//! chunk, so memory stays constant whatever the attachment size. The native store writes
//! files under `<data_dir>/blobs/` (`kb-storage-native`); [`MemoryBlobStore`] serves tests
//! and the in-memory mirror; the web store (OPFS) comes with the PWA milestone.
//!
//! Errors are [`StorageError::Blob`]: a message about what failed, never a path and
//! never a chunk's content.

use std::collections::HashMap;
use std::sync::Mutex;

use kb_types::{ChunkId, MaybeSend, MaybeSync};

use crate::StorageError;

/// Where encrypted chunks live. Implementations must be safe to call from the core's
/// single writer; they need no locking of their own beyond what `&self` requires.
pub trait BlobStore: MaybeSend + MaybeSync {
    /// Writes a chunk atomically (a reader never sees a partial chunk). Writing an id
    /// that already exists is a no-op: the content is the same by construction.
    fn put(&self, id: &ChunkId, bytes: &[u8]) -> Result<(), StorageError>;
    /// The chunk's bytes, `None` when it is not in the store.
    fn get(&self, id: &ChunkId) -> Result<Option<Vec<u8>>, StorageError>;
    /// Whether the chunk is in the store.
    fn has(&self, id: &ChunkId) -> Result<bool, StorageError>;
    /// Removes a chunk; removing one that is not there succeeds.
    fn delete(&self, id: &ChunkId) -> Result<(), StorageError>;
    /// Every chunk id in the store, in no particular order (the garbage collector's
    /// orphan sweep).
    fn list(&self) -> Result<Vec<ChunkId>, StorageError>;
    /// Removes the leftovers of interrupted writes (a write that never finished leaves a
    /// temporary file that no chunk id lists) and answers how many it removed. A store that
    /// writes atomically without temporary files has none, which is what this default says.
    fn sweep_temporaries(&self) -> Result<u32, StorageError> {
        Ok(0)
    }
}

/// In-memory store for tests and the in-memory mirror.
#[derive(Debug, Default)]
pub struct MemoryBlobStore {
    chunks: Mutex<HashMap<ChunkId, Vec<u8>>>,
}

impl MemoryBlobStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of chunks held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether the store holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total ciphertext bytes held.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.lock().values().map(|c| c.len() as u64).sum()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ChunkId, Vec<u8>>> {
        // A poisoned lock only means another test thread panicked mid-operation; the
        // map itself is still consistent (every operation is a single insert/remove).
        self.chunks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl BlobStore for MemoryBlobStore {
    fn put(&self, id: &ChunkId, bytes: &[u8]) -> Result<(), StorageError> {
        self.lock().entry(*id).or_insert_with(|| bytes.to_vec());
        Ok(())
    }

    fn get(&self, id: &ChunkId) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self.lock().get(id).cloned())
    }

    fn has(&self, id: &ChunkId) -> Result<bool, StorageError> {
        Ok(self.lock().contains_key(id))
    }

    fn delete(&self, id: &ChunkId) -> Result<(), StorageError> {
        self.lock().remove(id);
        Ok(())
    }

    fn list(&self) -> Result<Vec<ChunkId>, StorageError> {
        Ok(self.lock().keys().copied().collect())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn id(byte: u8) -> ChunkId {
        ChunkId::from_bytes([byte; 32])
    }

    #[test]
    fn memory_store_round_trips_and_is_idempotent() {
        let store = MemoryBlobStore::new();
        assert!(store.is_empty());
        store.put(&id(1), b"one").unwrap();
        store
            .put(&id(1), b"ignored: same id, same content by construction")
            .unwrap();
        store.put(&id(2), b"two").unwrap();
        assert_eq!(store.get(&id(1)).unwrap().unwrap(), b"one");
        assert!(store.has(&id(2)).unwrap());
        assert!(!store.has(&id(3)).unwrap());
        assert_eq!(store.get(&id(3)).unwrap(), None);
        assert_eq!(store.len(), 2);
        assert_eq!(store.bytes(), 6);
        let mut listed = store.list().unwrap();
        listed.sort();
        assert_eq!(listed, vec![id(1), id(2)]);
        store.delete(&id(1)).unwrap();
        store.delete(&id(9)).unwrap();
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn a_chunk_that_exists_keeps_its_first_content() {
        // The id is the hash of the content, so a second `put` carries the same bytes; the
        // store does not even look at them.
        let store = MemoryBlobStore::new();
        store.put(&id(1), b"first").unwrap();
        store.put(&id(1), b"second, under the same id").unwrap();
        assert_eq!(store.get(&id(1)).unwrap().unwrap(), b"first");
        assert_eq!(store.len(), 1);
        assert_eq!(store.bytes(), 5);
    }

    #[test]
    fn the_store_hands_out_copies() {
        let store = MemoryBlobStore::new();
        let mut bytes = b"original".to_vec();
        store.put(&id(1), &bytes).unwrap();
        // Neither the caller's buffer after the put nor a chunk that was read can reach
        // into the store.
        bytes.fill(0);
        let mut read = store.get(&id(1)).unwrap().unwrap();
        assert_eq!(read, b"original");
        read.fill(0xFF);
        assert_eq!(store.get(&id(1)).unwrap().unwrap(), b"original");
    }

    #[test]
    fn empty_and_chunk_sized_chunks_are_chunks_like_any_other() {
        let store = MemoryBlobStore::new();
        let full = vec![0xA5; 4 * 1024 * 1024 + 40];
        store.put(&id(1), b"").unwrap();
        store.put(&id(2), &full).unwrap();
        assert!(store.has(&id(1)).unwrap(), "an empty chunk exists");
        assert_eq!(store.get(&id(1)).unwrap().unwrap(), b"");
        assert_eq!(store.get(&id(2)).unwrap().unwrap(), full);
        assert_eq!(store.bytes(), 4 * 1024 * 1024 + 40);
        store.delete(&id(1)).unwrap();
        assert!(!store.has(&id(1)).unwrap());
        assert_eq!(store.get(&id(1)).unwrap(), None);
    }

    #[test]
    fn a_deleted_chunk_can_be_stored_again_and_list_follows() {
        let store = MemoryBlobStore::new();
        store.put(&id(1), b"one").unwrap();
        store.delete(&id(1)).unwrap();
        assert!(store.is_empty());
        assert!(store.list().unwrap().is_empty());
        assert_eq!(store.bytes(), 0);
        store.put(&id(1), b"one again").unwrap();
        assert_eq!(store.get(&id(1)).unwrap().unwrap(), b"one again");
        assert_eq!(store.list().unwrap(), [id(1)]);
    }

    #[test]
    fn the_store_works_through_a_trait_object_shared_between_threads() {
        let store: std::sync::Arc<dyn BlobStore> = std::sync::Arc::new(MemoryBlobStore::new());
        std::thread::scope(|scope| {
            for worker in 0..8u8 {
                let store = &store;
                scope.spawn(move || {
                    for n in 0..8u8 {
                        store.put(&id(worker * 8 + n), &[worker, n]).unwrap();
                    }
                });
            }
        });
        let mut listed = store.list().unwrap();
        listed.sort();
        assert_eq!(listed, (0..64).map(id).collect::<Vec<_>>());
        assert_eq!(store.get(&id(9)).unwrap().unwrap(), [1, 1]);
    }

    #[test]
    fn the_memory_store_has_no_temporaries_to_sweep() {
        let store = MemoryBlobStore::new();
        store.put(&id(1), b"one").unwrap();
        assert_eq!(store.sweep_temporaries().unwrap(), 0);
        let dynamic: &dyn BlobStore = &store;
        assert_eq!(dynamic.sweep_temporaries().unwrap(), 0);
        assert_eq!(
            store.get(&id(1)).unwrap().unwrap(),
            b"one",
            "nothing is touched"
        );
    }

    #[test]
    fn a_panic_in_another_thread_does_not_poison_the_store() {
        let store = std::sync::Arc::new(MemoryBlobStore::new());
        store.put(&id(1), b"one").unwrap();
        let crashed = {
            let store = std::sync::Arc::clone(&store);
            std::thread::spawn(move || {
                let _held = store.lock();
                std::panic::resume_unwind(Box::new("a test thread dies holding the lock"));
            })
            .join()
        };
        assert!(crashed.is_err());
        assert_eq!(store.get(&id(1)).unwrap().unwrap(), b"one");
        store.put(&id(2), b"two").unwrap();
        assert_eq!(store.len(), 2);
    }
}
