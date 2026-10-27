//! The native blob store: encrypted attachment chunks as files (ADR-0020).
//!
//! # Layout
//!
//! ```text
//! <root>/<first two hex chars>/<64 hex chars>.chunk
//! ```
//!
//! The hex is the lowercase [`ChunkId`] (BLAKE3 of the chunk's ciphertext), so a path
//! component never comes from anything but a chunk id: no user data, no name, no MIME type
//! ever reaches the file system. The files are ciphertext; their names are hashes that
//! say nothing about the attachment they belong to.
//!
//! # Writes
//!
//! A chunk is written to `<hex>.tmp` next to its final name, flushed to disk (`fsync`) and
//! renamed onto `<hex>.chunk`, so a reader sees the whole chunk or none of it, and a crash
//! leaves either nothing or a stray `.tmp`. A stray `.tmp` is overwritten by the next write
//! of the same chunk, removed by [`BlobStore::delete`] of it and by
//! [`BlobStore::sweep_temporaries`] (which only touches files named `<64 hex>.tmp` in their
//! own prefix directory); `list` never reports it.
//! On Unix the directory is flushed too (best effort), so the rename itself survives a power
//! cut. Writing a chunk that exists is a no-op: its content is the same by construction.
//!
//! The store is for the core's single writer (the [`BlobStore`] contract): two threads
//! writing the same chunk at the same moment would share one temporary file.
//!
//! # Memory
//!
//! Every operation handles one chunk. `put` writes the slice it is given, `get` reads one
//! file into one `Vec`, `list` holds the ids (32 bytes each); nothing buffers more than one
//! chunk, whatever the attachment size. The store imposes no size limit of its own: it is
//! dumb, and a sealed chunk can exceed `CHUNK_SIZE + 40` bytes (a final chunk whose padding
//! bucket rounds up).
//!
//! # Errors
//!
//! [`StorageError::Blob`] with what failed and the first eight hex characters of the chunk
//! id, plus the operating system's own message for the I/O error (which never carries a
//! path). Never a path, never a chunk's content.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use kb_storage::{BlobStore, StorageError};
use kb_types::ChunkId;

/// Extension of a stored chunk.
const CHUNK_SUFFIX: &str = ".chunk";

/// Extension of a chunk that is being written.
const TEMP_SUFFIX: &str = ".tmp";

/// Characters of the chunk id the directory is named after.
const PREFIX_LEN: usize = 2;

/// Characters of a chunk id as hex.
const HEX_LEN: usize = 64;

/// Chunks as files under one directory (see the [module documentation](self)).
pub struct FsBlobStore {
    root: PathBuf,
}

impl fmt::Debug for FsBlobStore {
    // No path in a log line or a panic message.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FsBlobStore").finish_non_exhaustive()
    }
}

impl FsBlobStore {
    /// A store rooted at `root`, which is created (with its parents) when missing.
    ///
    /// # Errors
    ///
    /// [`StorageError::Blob`] when the directory cannot be created.
    pub fn new(root: PathBuf) -> Result<Self, StorageError> {
        fs::create_dir_all(&root).map_err(|e| {
            StorageError::Blob(format!("cannot create the blob store directory: {e}"))
        })?;
        Ok(Self { root })
    }

    /// `<root>/<hh>`: the directory of a chunk.
    fn dir_of(&self, hex: &str) -> PathBuf {
        self.root.join(&hex[..PREFIX_LEN])
    }

    /// The final name of a chunk and the name it is written under.
    fn paths_of(&self, id: &ChunkId) -> (PathBuf, PathBuf, PathBuf) {
        let hex = id.to_hex();
        let dir = self.dir_of(&hex);
        let chunk = dir.join(format!("{hex}{CHUNK_SUFFIX}"));
        let temp = dir.join(format!("{hex}{TEMP_SUFFIX}"));
        (dir, chunk, temp)
    }
}

/// The error of a failed operation on a chunk: the action, the first eight characters of
/// the id and the system's message, never a path.
fn blob_error(action: &str, id: &ChunkId, err: &io::Error) -> StorageError {
    let hex = id.to_hex();
    StorageError::Blob(format!("cannot {action} chunk {}: {err}", &hex[..8]))
}

/// Writes `bytes` to `path` (replacing what is there) and flushes them to disk.
fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Flushes a directory so that a rename inside it survives a power cut. Unix only (Windows
/// cannot open a directory as a file) and best effort: the chunk is already complete and
/// flushed, and a rename that is lost only means the chunk is written again.
#[cfg(unix)]
fn sync_dir(dir: &Path) {
    if let Ok(handle) = File::open(dir) {
        let _ = handle.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) {}

impl BlobStore for FsBlobStore {
    fn put(&self, id: &ChunkId, bytes: &[u8]) -> Result<(), StorageError> {
        if self.has(id)? {
            return Ok(());
        }
        let (dir, chunk, temp) = self.paths_of(id);
        fs::create_dir_all(&dir).map_err(|e| blob_error("create the directory of", id, &e))?;
        // `File::create` truncates a stray temporary file of a write that never finished.
        let stored = write_synced(&temp, bytes).and_then(|()| fs::rename(&temp, &chunk));
        if let Err(err) = stored {
            let _ = fs::remove_file(&temp);
            return Err(blob_error("write", id, &err));
        }
        sync_dir(&dir);
        Ok(())
    }

    fn get(&self, id: &ChunkId) -> Result<Option<Vec<u8>>, StorageError> {
        let (_, chunk, _) = self.paths_of(id);
        match fs::read(&chunk) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(blob_error("read", id, &err)),
        }
    }

    fn has(&self, id: &ChunkId) -> Result<bool, StorageError> {
        let (_, chunk, _) = self.paths_of(id);
        match fs::metadata(&chunk) {
            Ok(meta) => Ok(meta.is_file()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(blob_error("look for", id, &err)),
        }
    }

    fn delete(&self, id: &ChunkId) -> Result<(), StorageError> {
        let (_, chunk, temp) = self.paths_of(id);
        // The half-written copy of an upload that was cut short goes with the chunk.
        for path in [&temp, &chunk] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(blob_error("delete", id, &err)),
            }
        }
        Ok(())
    }

    fn list(&self) -> Result<Vec<ChunkId>, StorageError> {
        let mut ids = Vec::new();
        self.walk(|prefix, entry| {
            if let Some(id) = stem_of(prefix, &entry.file_name(), CHUNK_SUFFIX) {
                ids.push(id);
            }
        })?;
        Ok(ids)
    }

    fn sweep_temporaries(&self) -> Result<u32, StorageError> {
        let mut leftovers = Vec::new();
        self.walk(|prefix, entry| {
            if stem_of(prefix, &entry.file_name(), TEMP_SUFFIX).is_some() {
                leftovers.push(entry.path());
            }
        })?;
        // Removed after the walk, so that no directory is read while it changes.
        let mut removed = 0;
        for path in leftovers {
            match fs::remove_file(&path) {
                Ok(()) => removed += 1,
                // Gone since it was found: not ours to count.
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(StorageError::Blob(format!(
                        "cannot remove the leftover of an interrupted write: {err}"
                    )));
                }
            }
        }
        Ok(removed)
    }
}

impl FsBlobStore {
    /// Calls `visit(prefix, entry)` for every regular file in every prefix directory (the
    /// two levels of the layout). Everything else is skipped without a word: files at the
    /// root, directories that are not two lowercase hex characters, subdirectories. A store
    /// whose directory is gone holds nothing (the next write recreates it).
    fn walk(&self, mut visit: impl FnMut(&str, &fs::DirEntry)) -> Result<(), StorageError> {
        let walk_error =
            |e: &io::Error| StorageError::Blob(format!("cannot list the blob store: {e}"));
        let prefixes = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(walk_error(&err)),
        };
        for prefix in prefixes {
            let prefix = prefix.map_err(|e| walk_error(&e))?;
            let Some(prefix_name) = valid_prefix(&prefix.file_name()) else {
                continue;
            };
            if !prefix.file_type().map_err(|e| walk_error(&e))?.is_dir() {
                continue;
            }
            let files = match fs::read_dir(prefix.path()) {
                Ok(entries) => entries,
                // Removed since the walk of the root began.
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(walk_error(&err)),
            };
            for entry in files {
                let entry = entry.map_err(|e| walk_error(&e))?;
                // Only regular files count (a directory with a chunk's name is foreign).
                if entry.file_type().map_err(|e| walk_error(&e))?.is_file() {
                    visit(&prefix_name, &entry);
                }
            }
        }
        Ok(())
    }
}

/// The name of a prefix directory: two lowercase hex characters.
fn valid_prefix(name: &std::ffi::OsStr) -> Option<String> {
    let name = name.to_str()?;
    (name.len() == PREFIX_LEN && is_lower_hex(name)).then(|| name.to_owned())
}

/// The id of a file named `<64 lowercase hex><suffix>` in the directory `prefix` (the suffix
/// is [`CHUNK_SUFFIX`] for a chunk and [`TEMP_SUFFIX`] for a write in progress); anything
/// else (the other suffix, a stray file, a file in the wrong directory, uppercase hex) is
/// not a file of this store.
fn stem_of(prefix: &str, file_name: &std::ffi::OsStr, suffix: &str) -> Option<ChunkId> {
    let hex = file_name.to_str()?.strip_suffix(suffix)?;
    if hex.len() != HEX_LEN || !is_lower_hex(hex) || !hex.starts_with(prefix) {
        return None;
    }
    ChunkId::from_hex(hex).ok()
}

fn is_lower_hex(text: &str) -> bool {
    text.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn store() -> (tempfile::TempDir, FsBlobStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path().join("blobs")).unwrap();
        (dir, store)
    }

    /// An id like the real ones: the hash of the content.
    fn id_of(bytes: &[u8]) -> ChunkId {
        ChunkId::from_bytes(kb_crypto::hash(bytes))
    }

    fn sorted(mut ids: Vec<ChunkId>) -> Vec<ChunkId> {
        ids.sort();
        ids
    }

    /// Fills `buf` with pseudo-random bytes (SplitMix64) that depend on `seed` only: the
    /// same chunk can be produced again to compare with what was read back, and nothing
    /// but the one buffer is ever held.
    fn fill_chunk(buf: &mut [u8], seed: u64) {
        let mut state = seed;
        for word in buf.chunks_mut(8) {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            word.copy_from_slice(&z.to_le_bytes()[..word.len()]);
        }
    }

    // ---- the basics ---------------------------------------------------------------

    #[test]
    fn new_creates_the_root_with_its_parents_and_accepts_one_that_exists() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("a").join("b").join("blobs");
        let store = FsBlobStore::new(root.clone()).unwrap();
        assert!(root.is_dir());
        assert!(store.list().unwrap().is_empty());
        // A second store over the same directory sees what the first wrote.
        let chunk = id_of(b"shared");
        store.put(&chunk, b"shared").unwrap();
        let again = FsBlobStore::new(root).unwrap();
        assert_eq!(again.get(&chunk).unwrap().unwrap(), b"shared");
    }

    #[test]
    fn new_fails_with_a_typed_error_and_no_path_when_the_root_is_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let occupied = dir.path().join("occupied");
        fs::write(&occupied, "x").unwrap();
        let err = FsBlobStore::new(occupied.clone()).unwrap_err();
        let StorageError::Blob(message) = &err else {
            unreachable!("{err}")
        };
        assert!(
            !message.contains(&occupied.display().to_string()),
            "{message}"
        );
        assert!(
            !message.contains(dir.path().to_str().unwrap()),
            "no path: {message}"
        );
    }

    #[test]
    fn a_chunk_round_trips_and_lands_where_the_layout_says() {
        let (_dir, store) = store();
        let bytes: Vec<u8> = (0..=255).cycle().take(10_000).collect();
        let id = id_of(&bytes);
        assert!(!store.has(&id).unwrap());
        assert_eq!(store.get(&id).unwrap(), None);

        store.put(&id, &bytes).unwrap();
        assert!(store.has(&id).unwrap());
        assert_eq!(store.get(&id).unwrap().unwrap(), bytes);

        let hex = id.to_hex();
        let path = store.root.join(&hex[..2]).join(format!("{hex}.chunk"));
        assert_eq!(fs::read(&path).unwrap(), bytes, "<root>/<hh>/<hex>.chunk");
        let siblings: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(siblings.len(), 1, "no temporary file is left: {siblings:?}");
        assert_eq!(store.list().unwrap(), [id]);
    }

    #[test]
    fn empty_and_chunk_sized_chunks_round_trip() {
        let (_dir, store) = store();
        let empty = id_of(b"");
        store.put(&empty, b"").unwrap();
        assert_eq!(store.get(&empty).unwrap().unwrap(), b"");
        assert!(store.has(&empty).unwrap());

        // A full chunk, a full chunk and its overhead, and the 8 MiB bucket a final chunk
        // can round up to.
        for len in [4 * 1024 * 1024, 4 * 1024 * 1024 + 40, 8 * 1024 * 1024 + 40] {
            let mut bytes = vec![0; len];
            fill_chunk(&mut bytes, len as u64);
            let id = id_of(&bytes);
            store.put(&id, &bytes).unwrap();
            assert_eq!(store.get(&id).unwrap().unwrap(), bytes, "{len} bytes");
        }
        assert_eq!(store.list().unwrap().len(), 4);
    }

    #[test]
    fn putting_a_chunk_that_exists_is_a_no_op() {
        let (_dir, store) = store();
        let id = id_of(b"first");
        store.put(&id, b"first").unwrap();
        let hex = id.to_hex();
        let path = store.root.join(&hex[..2]).join(format!("{hex}.chunk"));
        let written = fs::metadata(&path).unwrap().modified().unwrap();

        // The id says the content is the same, so the file is not touched, whatever the
        // bytes of the second call are.
        store.put(&id, b"first").unwrap();
        store
            .put(&id, b"different bytes under the same id")
            .unwrap();
        assert_eq!(store.get(&id).unwrap().unwrap(), b"first");
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), written);
        assert_eq!(store.list().unwrap(), [id]);
    }

    #[test]
    fn delete_removes_the_chunk_and_a_missing_chunk_is_not_an_error() {
        let (_dir, store) = store();
        let (a, b) = (id_of(b"a"), id_of(b"b"));
        store.put(&a, b"a").unwrap();
        store.put(&b, b"b").unwrap();

        store.delete(&a).unwrap();
        assert!(!store.has(&a).unwrap());
        assert_eq!(store.get(&a).unwrap(), None);
        assert_eq!(store.get(&b).unwrap().unwrap(), b"b", "another chunk stays");
        assert_eq!(store.list().unwrap(), [b]);

        // Again, and a chunk that never was; also in a prefix directory that does not exist.
        store.delete(&a).unwrap();
        store.delete(&id_of(b"never stored")).unwrap();
        store.delete(&ChunkId::from_bytes([0xEE; 32])).unwrap();
        assert_eq!(store.list().unwrap(), [b]);

        // A deleted chunk can be stored again.
        store.put(&a, b"a").unwrap();
        assert_eq!(sorted(store.list().unwrap()), sorted(vec![a, b]));
    }

    #[test]
    fn list_walks_every_prefix_directory() {
        let (_dir, store) = store();
        // Ids that start with different bytes land in different directories.
        let ids: Vec<ChunkId> = (0..20u8)
            .map(|n| ChunkId::from_bytes(std::array::from_fn(|i| n.wrapping_mul(13) ^ i as u8)))
            .collect();
        for id in &ids {
            store.put(id, &id.to_bytes()).unwrap();
        }
        let dirs = fs::read_dir(&store.root).unwrap().count();
        assert!(dirs > 10, "the chunks are spread over {dirs} directories");
        assert_eq!(sorted(store.list().unwrap()), sorted(ids));
    }

    #[test]
    fn list_of_a_store_whose_directory_is_gone_is_empty_and_the_next_write_recreates_it() {
        let (_dir, store) = store();
        fs::remove_dir_all(&store.root).unwrap();
        assert!(store.list().unwrap().is_empty());
        assert!(!store.has(&id_of(b"x")).unwrap());
        assert_eq!(store.get(&id_of(b"x")).unwrap(), None);
        store.delete(&id_of(b"x")).unwrap();
        store.put(&id_of(b"x"), b"x").unwrap();
        assert_eq!(store.get(&id_of(b"x")).unwrap().unwrap(), b"x");
    }

    // ---- foreign files and atomicity -----------------------------------------------

    #[test]
    fn list_ignores_files_that_are_not_chunks() {
        let (_dir, store) = store();
        let id = id_of(b"real");
        store.put(&id, b"real").unwrap();
        let hex = id.to_hex();
        let prefix_name = &hex[..2];
        let prefix = store.root.join(prefix_name);
        // Two prefixes that are not the real one (the names are checked below, never
        // compared with a real chunk's: the file system may ignore case).
        let others: Vec<&str> = ["00", "11", "22"]
            .into_iter()
            .filter(|p| *p != prefix_name)
            .collect();
        let (wrong, file_prefix) = (others[0], others[1]);

        // In the right directory: a temporary file, another extension, none, a short and a
        // long name, uppercase hex, a chunk that belongs to another prefix, nothing but the
        // extension, a text file, and a directory with a valid chunk name.
        let uppercase = format!("{}{}", prefix_name.to_uppercase(), "A".repeat(62));
        let wrong_prefix = format!("{wrong}{}", "7".repeat(62));
        for name in [
            format!("{hex}.tmp"),
            format!("{hex}.bin"),
            hex.clone(),
            format!("{}.chunk", &hex[..63]),
            format!("{hex}0.chunk"),
            format!("{uppercase}.chunk"),
            format!("{wrong_prefix}.chunk"),
            ".chunk".to_owned(),
            "notes.txt".to_owned(),
        ] {
            fs::write(prefix.join(&name), b"foreign").unwrap();
        }
        fs::create_dir(prefix.join(format!("{prefix_name}{}.chunk", "9".repeat(62)))).unwrap();

        // At the root: files and directories that are not a prefix directory, a directory
        // of a prefix with a stray chunk in it, and a plain file where a prefix would be.
        fs::write(store.root.join("readme.txt"), b"x").unwrap();
        fs::write(store.root.join("ab.chunk"), b"x").unwrap();
        fs::write(store.root.join("zz"), b"x").unwrap();
        fs::create_dir(store.root.join("zz2")).unwrap();
        fs::create_dir(store.root.join(wrong)).unwrap();
        fs::write(
            store
                .root
                .join(wrong)
                .join(format!("{wrong}{}.tmp", "5".repeat(62))),
            b"temp",
        )
        .unwrap();
        fs::write(store.root.join(file_prefix), b"x").unwrap();

        assert_eq!(store.list().unwrap(), [id], "only the real chunk is listed");
        assert_eq!(store.get(&id).unwrap().unwrap(), b"real");
    }
    #[test]
    fn a_stray_temporary_file_is_replaced_by_the_write_and_never_listed() {
        let (_dir, store) = store();
        let bytes = b"the real chunk".to_vec();
        let id = id_of(&bytes);
        let (dir, chunk, temp) = store.paths_of(&id);
        fs::create_dir_all(&dir).unwrap();
        // What a crash in the middle of a write leaves: a temporary file with part of the
        // content, much longer garbage even.
        fs::write(&temp, vec![0xAB; 50_000]).unwrap();
        assert!(
            store.list().unwrap().is_empty(),
            "a temporary file is not a chunk"
        );
        assert!(!store.has(&id).unwrap());
        assert_eq!(store.get(&id).unwrap(), None);

        store.put(&id, &bytes).unwrap();
        assert_eq!(store.get(&id).unwrap().unwrap(), bytes);
        assert_eq!(fs::read(&chunk).unwrap(), bytes);
        assert!(!temp.exists(), "the leftover was replaced and renamed away");
        assert_eq!(store.list().unwrap(), [id]);
    }

    #[test]
    fn sweep_removes_stray_temporary_files_counts_them_and_leaves_everything_else() {
        let (_dir, store) = store();
        // Chunks in several prefix directories, which must all stay.
        let chunks: Vec<ChunkId> = (0..6u8).map(|n| id_of(&[n; 3])).collect();
        for (n, id) in (0..6u8).zip(&chunks) {
            store.put(id, &[n; 3]).unwrap();
        }
        assert_eq!(
            store.sweep_temporaries().unwrap(),
            0,
            "nothing to sweep yet"
        );

        // The leftovers of three interrupted writes: one beside a chunk of the same
        // prefix, one for a chunk that was never completed, one in a prefix of its own.
        let beside = store.paths_of(&chunks[0]).2;
        let alone = store.paths_of(&id_of(b"never completed")).2;
        let elsewhere = store.paths_of(&id_of(b"cut short")).2;
        for temp in [&beside, &alone, &elsewhere] {
            fs::create_dir_all(temp.parent().unwrap()).unwrap();
            fs::write(temp, vec![0xAB; 1_000]).unwrap();
        }
        // Foreign files that happen to end in `.tmp`, and shapes that are not files: a
        // short stem, uppercase hex, a file in the wrong prefix directory, a directory with
        // a temporary file's name, a file at the root and in a directory that is no prefix.
        let prefix_dir = beside.parent().unwrap().to_owned();
        let prefix_name = prefix_dir.file_name().unwrap().to_str().unwrap().to_owned();
        let wrong = if prefix_name == "00" { "11" } else { "00" };
        let foreign = [
            prefix_dir.join("notes.tmp"),
            prefix_dir.join(format!("{}.tmp", "a".repeat(63))),
            prefix_dir.join(format!("{prefix_name}{}.tmp", "A".repeat(62))),
            prefix_dir.join(format!("{wrong}{}.tmp", "7".repeat(62))),
            store.root.join("root.tmp"),
        ];
        for path in &foreign {
            fs::write(path, b"foreign").unwrap();
        }
        let dir_named_like_a_temp = prefix_dir.join(format!("{prefix_name}{}.tmp", "9".repeat(62)));
        fs::create_dir(&dir_named_like_a_temp).unwrap();
        fs::create_dir_all(store.root.join("zz")).unwrap();
        fs::write(
            store
                .root
                .join("zz")
                .join(format!("{}.tmp", "b".repeat(64))),
            b"x",
        )
        .unwrap();

        assert_eq!(store.sweep_temporaries().unwrap(), 3);
        for temp in [&beside, &alone, &elsewhere] {
            assert!(!temp.exists(), "the leftover is gone");
        }
        for path in &foreign {
            assert!(path.exists(), "a foreign file stays");
        }
        assert!(dir_named_like_a_temp.is_dir());
        assert!(
            store
                .root
                .join("zz")
                .join(format!("{}.tmp", "b".repeat(64)))
                .exists()
        );
        for (n, id) in (0..6u8).zip(&chunks) {
            assert_eq!(store.get(id).unwrap().unwrap(), [n; 3], "the chunks stay");
        }
        assert_eq!(sorted(store.list().unwrap()), sorted(chunks));

        // Nothing is left to find the second time, and a missing store sweeps nothing.
        assert_eq!(store.sweep_temporaries().unwrap(), 0);
        fs::remove_dir_all(&store.root).unwrap();
        assert_eq!(store.sweep_temporaries().unwrap(), 0);
    }

    #[test]
    fn delete_also_removes_a_stray_temporary_file() {
        let (_dir, store) = store();
        let id = id_of(b"cut short");
        let (dir, chunk, temp) = store.paths_of(&id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(&temp, b"half").unwrap();
        store.delete(&id).unwrap();
        assert!(!temp.exists());
        assert!(!chunk.exists());

        // Both at once.
        fs::write(&temp, b"half").unwrap();
        store.put(&id, b"whole").unwrap();
        fs::write(&temp, b"half").unwrap();
        store.delete(&id).unwrap();
        assert!(!temp.exists() && !chunk.exists());
    }

    #[test]
    fn a_failed_write_leaves_no_temporary_file_and_no_chunk() {
        let (_dir, store) = store();
        let id = id_of(b"cannot be renamed");
        let (dir, chunk, temp) = store.paths_of(&id);
        fs::create_dir_all(&dir).unwrap();
        // A directory sits where the chunk file goes: the rename onto it fails.
        fs::create_dir(&chunk).unwrap();

        let err = store.put(&id, b"cannot be renamed").unwrap_err();
        assert!(matches!(err, StorageError::Blob(_)), "{err}");
        assert!(!temp.exists(), "the temporary file is cleaned up");
        assert!(chunk.is_dir(), "what was there is untouched");
        assert!(store.list().unwrap().is_empty());
        assert!(!store.has(&id).unwrap(), "a directory is not a chunk");
    }

    #[test]
    fn errors_name_what_failed_with_a_short_id_and_never_a_path_or_content() {
        let (dir, store) = store();
        let content = b"SECRET-CHUNK-CONTENT";
        let id = id_of(content);
        // A regular file where the prefix directory belongs makes every write fail.
        let hex = id.to_hex();
        fs::write(store.root.join(&hex[..2]), b"in the way").unwrap();

        let err = store.put(&id, content).unwrap_err();
        let StorageError::Blob(message) = &err else {
            unreachable!("{err}")
        };
        assert!(
            message.contains("write") || message.contains("create"),
            "{message}"
        );
        assert!(message.contains(&hex[..8]), "{message}");
        assert!(!message.contains(&hex[..9]), "only 8 characters: {message}");
        assert!(!message.contains(&hex), "{message}");
        assert!(!message.contains("SECRET"), "{message}");
        let root = store.root.display().to_string();
        let tmp = dir.path().display().to_string();
        assert!(!message.contains(&root), "{message}");
        assert!(!message.contains(&tmp), "no path: {message}");
        assert!(
            !message.contains(".chunk") && !message.contains(".tmp"),
            "{message}"
        );
        assert!(err.to_string().starts_with("blob store error: "), "{err}");

        // The same for a read of a chunk that cannot be read (a directory in its place).
        let other = id_of(b"other");
        let (other_dir, other_chunk, _) = store.paths_of(&other);
        fs::create_dir_all(&other_dir).unwrap();
        fs::create_dir(&other_chunk).unwrap();
        let err = store.get(&other).unwrap_err();
        let message = err.to_string();
        assert!(matches!(err, StorageError::Blob(_)), "{message}");
        assert!(!message.contains(&tmp), "no path: {message}");
        assert!(!message.contains(&other.to_hex()), "{message}");
    }

    #[test]
    fn debug_output_has_no_path() {
        let (dir, store) = store();
        let text = format!("{store:?}");
        assert!(text.contains("FsBlobStore"), "{text}");
        assert!(!text.contains(dir.path().to_str().unwrap()), "{text}");
    }

    #[test]
    fn it_works_through_a_trait_object_and_from_several_threads() {
        let dir = tempfile::tempdir().unwrap();
        let store: std::sync::Arc<dyn BlobStore> =
            std::sync::Arc::new(FsBlobStore::new(dir.path().join("blobs")).unwrap());
        // Distinct chunks in parallel (the shared temporary file only matters for one id).
        std::thread::scope(|scope| {
            for worker in 0..8u8 {
                let store = &store;
                scope.spawn(move || {
                    for n in 0..8u8 {
                        let bytes = vec![worker, n, worker ^ n];
                        store.put(&id_of(&bytes), &bytes).unwrap();
                    }
                });
            }
        });
        let ids = store.list().unwrap();
        assert_eq!(ids.len(), 64);
        for worker in 0..8u8 {
            for n in 0..8u8 {
                let bytes = vec![worker, n, worker ^ n];
                assert_eq!(store.get(&id_of(&bytes)).unwrap().unwrap(), bytes);
            }
        }
    }

    // ---- streams of chunks ---------------------------------------------------------

    /// Writes `chunks` chunks of 4 MiB of pseudo-random bytes one after the other, then
    /// reads every one back and compares it with the bytes it was made from, then deletes
    /// them. At any moment the test holds one buffer to generate into (4 MiB) and the one
    /// the store hands back (4 MiB), plus the 32-byte ids: memory does not grow with the
    /// total, and the store itself keeps nothing between calls.
    fn stream_of_chunks_round_trips(chunks: u64) {
        const CHUNK: usize = 4 * 1024 * 1024;
        let (_dir, store) = store();
        let mut buf = vec![0; CHUNK];
        let mut ids = Vec::new();

        for seed in 0..chunks {
            fill_chunk(&mut buf, seed);
            let id = id_of(&buf);
            store.put(&id, &buf).unwrap();
            ids.push(id);
        }
        assert_eq!(
            store.list().unwrap().len(),
            ids.len(),
            "every chunk is distinct"
        );

        for (seed, id) in (0..chunks).zip(&ids) {
            fill_chunk(&mut buf, seed);
            let read = store.get(id).unwrap().unwrap();
            assert_eq!(read.len(), CHUNK, "chunk {seed}");
            assert!(read == buf, "chunk {seed} comes back as it went in");
            assert_eq!(id_of(&read), *id, "chunk {seed} still hashes to its id");
        }

        for id in &ids {
            store.delete(id).unwrap();
        }
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn a_stream_of_64_mb_of_random_chunks_round_trips_one_chunk_at_a_time() {
        stream_of_chunks_round_trips(16);
    }

    /// The real-pass test of milestone 5: 500 MiB through the real file store. Slow in a
    /// debug build (BLAKE3 and the disk), so it is skipped by default. Run it with
    ///
    /// ```text
    /// cargo test -p kb-storage-native --release -- --ignored
    /// ```
    ///
    /// and read the process's peak working set from the outside (the test cannot: the
    /// workspace forbids `unsafe`), for instance by polling `PeakWorkingSet64` of the test
    /// executable from PowerShell. Measured on the development machine (Windows 11, release
    /// build): 500 MiB in about 2 s with a peak working set of about 22 MiB, a few buffers of
    /// one chunk plus the test harness.
    #[test]
    #[ignore = "writes 500 MiB; run with `cargo test -p kb-storage-native --release -- --ignored`"]
    #[allow(clippy::print_stderr)]
    fn a_stream_of_500_mb_of_random_chunks_round_trips_one_chunk_at_a_time() {
        let started = Instant::now();
        stream_of_chunks_round_trips(125);
        eprintln!(
            "500 MiB written, read back and deleted in {:.1} s",
            started.elapsed().as_secs_f64()
        );
    }
}
