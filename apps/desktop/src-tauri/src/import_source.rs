//! Where an import reads from (ADR-0036, `docs/architecture/import-export.md`, 3.2, 3.9 and 5.2):
//! some picked files, a folder, or a `.zip` such as a Notion or atomik-kb export. An
//! [`ImportSource`] lists the files of the source by their relative, `/`-separated path (text files
//! apart from the others) without reading any of them, and then reads one at a time on request, so
//! a large source never sits in memory.
//!
//! - **Text files** are those with the extension `.md`, `.markdown`, `.txt`, `.html`, `.htm`,
//!   `.csv` or `.excalidraw`, and `atomik-export.json` at the root; everything else is a **binary**
//!   path, which the core may ask for as an attachment ([`ImportSource::with_binary`]).
//! - **Hidden entries** (a segment starting with `.`), `__MACOSX` and `node_modules` are skipped
//!   without a word, and so are folders, links and special files. Anything else that is left out
//!   is reported as an [`ImportWarning`] (`UnsupportedFile`, `Truncated`).
//! - **Zip slip**: an archive entry whose name is not a safe relative path (see `archive_path`:
//!   absolute, `..`, a backslash, a drive letter, a control character) never reaches the core, and
//!   a zip is never extracted to disk by its entries' names, so a crafted name cannot write
//!   anywhere. The one thing written is the nested zip below, to a name of our own choosing.
//! - **One nested zip level**: a `.zip` at the root of the archive (Notion's `Part-N.zip`) is
//!   extracted to a temporary file in the app's cache folder, opened, and its entries are merged
//!   into the same tree. A zip inside that one, or at any other place, is an ordinary file. The
//!   temporary files are deleted when the source is dropped (and the whole folder when the app
//!   starts, [`clear_import_dir`]).
//! - **Text decoding** ([`decode_text`]): UTF-8 (a byte order mark removed), UTF-16 with a byte
//!   order mark, otherwise UTF-8 with replacement characters, which the caller reports as
//!   `NotUtf8`. At most [`MAX_TEXT_BYTES`] per file, checked on the declared size and while
//!   reading.
//!
//! - **An AppFlowy backup** (ADR-0039, "Back up your workspace"): a folder or `.zip` whose root
//!   (after one wrapping folder) holds `relation_map.json` and `metadata.json`. Every `.json` file
//!   under that root is then a text file (the reader needs them), except the attached files under
//!   `local_files/`, and everything under `collab_objects/` (binary copies the importer never
//!   reads) is skipped without a word. Any source type can be imported into new spaces; only
//!   an AppFlowy backup brings its own spaces.
//!
//! Limits: a folder is walked to 100,000 files and 64 levels, the sibling files of picked files to
//! 20,000 files and 4 levels, an archive reads 100,000 entries, a binary path list holds 100,000.
//! Folders are never followed through links. No path or name is ever logged.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use kb_types::{ApiError, ImportWarning, ImportWarningCode};
use zip::ZipArchive;
use zip::result::ZipError;

use crate::archive_path::{self, PathProblem};
use crate::backup::path_error;

/// The largest text file the core takes (and the shell reads).
pub const MAX_TEXT_BYTES: u64 = 16 * 1024 * 1024;

/// The most binary paths an import lists (the core refuses more).
pub const MAX_BINARY_PATHS: usize = 100_000;

/// The file that carries an atomik-kb export's property schema and page order, at the root.
pub const SIDECAR_NAME: &str = "atomik-export.json";

/// The two files that make a folder an AppFlowy backup (ADR-0039), and the two folders of it the
/// shell treats apart.
const RELATION_MAP_NAME: &str = "relation_map.json";
const METADATA_NAME: &str = "metadata.json";
const COLLAB_OBJECTS_DIR: &str = "collab_objects";
const LOCAL_FILES_DIR: &str = "local_files";

/// Extensions of the files the importer reads as text (lowercase).
const TEXT_EXTENSIONS: [&str; 7] = ["md", "markdown", "txt", "html", "htm", "csv", "excalidraw"];

/// Folder of the temporary files of nested zips, inside the app's cache folder.
pub const IMPORT_DIR: &str = "import";

/// Files read next to the picked ones, and how deep.
const SIBLING_MAX_FILES: usize = 20_000;
const SIBLING_MAX_DEPTH: usize = 4;

/// The limits of a folder source.
const FOLDER_MAX_FILES: usize = 100_000;
const FOLDER_MAX_DEPTH: usize = 64;

/// The most entries read from one archive.
const ZIP_MAX_ENTRIES: usize = 100_000;

/// The largest nested zip that is extracted (a `Part-N.zip` of a big workspace).
const NESTED_ZIP_MAX_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// The longest path shown in a warning, in characters.
const WARNING_PATH_CHARS: usize = 300;

/// Makes the names of temporary files unique within a process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Whether `path` (relative, `/`-separated) is a text file for the importer.
#[must_use]
pub fn is_text_path(path: &str) -> bool {
    if path == SIDECAR_NAME {
        return true;
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        TEXT_EXTENSIONS
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known))
    })
}

/// Where a file's bytes are.
#[derive(Debug, Clone)]
enum Origin {
    /// A file on disk.
    Disk(PathBuf),
    /// An entry of one of the source's open archives.
    Zip { archive: usize, index: usize },
}

/// A file of the source: its path as the core will see it, its size as declared, and where to
/// read it.
#[derive(Debug, Clone)]
pub struct SourceFile {
    path: String,
    size: u64,
    origin: Origin,
}

impl SourceFile {
    /// The relative, `/`-separated path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The size in bytes the file system or the archive declares.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }
}

/// What reading a text file gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextRead {
    /// The decoded text; `lossy` when it was not valid text and replacement characters stand in.
    Text {
        /// The text, a byte order mark removed.
        text: String,
        /// Replacement characters were used (`NotUtf8`).
        lossy: bool,
    },
    /// Above [`MAX_TEXT_BYTES`] (`TooLarge`).
    TooLarge,
    /// Could not be read (a file that vanished, a damaged archive entry).
    Unreadable,
}

/// The files of an import source. See the module documentation.
pub struct ImportSource {
    name: Option<String>,
    text: Vec<SourceFile>,
    binary: BTreeMap<String, SourceFile>,
    seen: HashSet<String>,
    warnings: Vec<ImportWarning>,
    binary_truncated: bool,
    // Closed before the temporary files they read are removed (see `Drop`).
    archives: Vec<ZipArchive<File>>,
    temp_files: Vec<PathBuf>,
}

impl Drop for ImportSource {
    fn drop(&mut self) {
        // Windows will not delete a file that is open.
        self.archives.clear();
        for path in self.temp_files.drain(..) {
            let _ = fs::remove_file(path);
        }
    }
}

impl ImportSource {
    fn new(name: Option<String>) -> Self {
        Self {
            name,
            text: Vec::new(),
            binary: BTreeMap::new(),
            seen: HashSet::new(),
            warnings: Vec::new(),
            binary_truncated: false,
            archives: Vec::new(),
            temp_files: Vec::new(),
        }
    }

    // --- construction ---

    /// The source of files picked one by one: the root is their common folder, the text files are
    /// the picked ones (a picked file that is not a text type is left out with `UnsupportedFile`),
    /// and the binary paths are the other files under the root, down to 4 levels and at most
    /// 20,000 of them. There is no source name.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for the field `path` when no file is given or the files share no folder.
    pub fn from_files(picked: &[PathBuf]) -> Result<Self, ApiError> {
        if picked.is_empty() {
            return Err(ApiError::invalid("path", "no file was chosen"));
        }
        let root = common_folder(picked)
            .ok_or_else(|| ApiError::invalid("path", "the files are not in one folder"))?;
        let mut source = Self::new(None);
        let mut picked_keys = HashSet::new();
        for file in picked {
            let Some(relative) = relative_text(&root, file) else {
                source.warn(&lossy(file), ImportWarningCode::UnsupportedFile, "name");
                continue;
            };
            if let Some(problem) = archive_path::path_problem(&relative) {
                source.warn_problem(&relative, problem);
                continue;
            }
            if !is_text_path(&relative) {
                source.warn(&relative, ImportWarningCode::UnsupportedFile, "type");
                continue;
            }
            match fs::metadata(file) {
                Ok(metadata) if metadata.is_file() => {
                    picked_keys.insert(path_key(&relative));
                    source.insert(
                        SourceFile {
                            path: relative,
                            size: metadata.len(),
                            origin: Origin::Disk(file.clone()),
                        },
                        true,
                    );
                }
                _ => source.warn(&relative, ImportWarningCode::UnsupportedFile, "unreadable"),
            }
        }
        let walk = walk_tree(&root, SIBLING_MAX_DEPTH, SIBLING_MAX_FILES);
        source.take_walk_problems(&walk);
        for found in walk.files {
            if picked_keys.contains(&path_key(&found.relative)) {
                continue;
            }
            source.insert(found.into_file(), false);
        }
        if walk.truncated {
            source.warn("", ImportWarningCode::Truncated, "files");
        }
        source.finish();
        Ok(source)
    }

    /// The source of a folder: every file under it (links are not followed, and hidden entries
    /// are skipped), at most 100,000 files and 64 levels deep. The source name is the folder's.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for the field `path` when the folder cannot be opened.
    pub fn from_folder(folder: &Path) -> Result<Self, ApiError> {
        fs::read_dir(folder).map_err(|error| path_error("the folder cannot be opened", &error))?;
        let name = folder
            .file_name()
            .and_then(OsStr::to_str)
            .map(str::to_owned)
            .filter(|name| !name.is_empty());
        let mut source = Self::new(name);
        let walk = walk_tree(folder, FOLDER_MAX_DEPTH, FOLDER_MAX_FILES);
        source.take_walk_problems(&walk);
        for found in walk.files {
            let text = is_text_path(&found.relative);
            source.insert(found.into_file(), text);
        }
        if walk.truncated {
            source.warn("", ImportWarningCode::Truncated, "files");
        }
        source.adopt_appflowy_backup();
        source.finish();
        Ok(source)
    }

    /// The source of a `.zip`: its entries (the nested zips of its root merged in, see the module
    /// documentation), read in place, never extracted by their names. `cache_dir` is where a
    /// nested zip is unpacked to read it. The source name is the zip's file name without its
    /// extension.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for the field `path` when the file is not a zip archive or cannot be opened,
    /// `Storage` when it cannot be read.
    pub fn from_zip(zip_path: &Path, cache_dir: &Path) -> Result<Self, ApiError> {
        let file = File::open(zip_path)
            .map_err(|error| path_error("the zip file cannot be opened", &error))?;
        let archive = ZipArchive::new(file).map_err(|error| match error {
            ZipError::Io(error) => {
                ApiError::storage(format!("the zip file could not be read: {error}"))
            }
            _ => ApiError::invalid("path", "the file is not a valid zip archive"),
        })?;
        let name = zip_path
            .file_stem()
            .and_then(OsStr::to_str)
            .map(str::to_owned)
            .filter(|name| !name.is_empty());
        let mut source = Self::new(name);
        source.add_archive(archive, true, cache_dir);
        source.adopt_appflowy_backup();
        source.finish();
        Ok(source)
    }

    // --- what the source holds ---

    /// The name of the source for `ImportBegin`: the folder's, or the zip's without extension;
    /// none for picked files.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The text files, in path order.
    #[must_use]
    pub fn text_files(&self) -> Vec<SourceFile> {
        self.text.clone()
    }

    /// The paths of all the other files, in path order (what `ImportPlan` takes).
    #[must_use]
    pub fn binary_paths(&self) -> Vec<String> {
        self.binary.keys().cloned().collect()
    }

    /// The problems found while listing the files, in the order found; empties the list.
    pub fn take_warnings(&mut self) -> Vec<ImportWarning> {
        std::mem::take(&mut self.warnings)
    }

    // --- reading ---

    /// Reads and decodes a text file (see the module documentation). Never fails: a file that
    /// cannot be read is [`TextRead::Unreadable`] and the caller leaves it out.
    pub fn read_text(&mut self, file: &SourceFile) -> TextRead {
        if file.size > MAX_TEXT_BYTES {
            return TextRead::TooLarge;
        }
        let Ok(bytes) = self.read_limited(file, MAX_TEXT_BYTES + 1) else {
            return TextRead::Unreadable;
        };
        if bytes.len() as u64 > MAX_TEXT_BYTES {
            return TextRead::TooLarge;
        }
        let (text, lossy) = decode_text(&bytes);
        if text.len() as u64 > MAX_TEXT_BYTES {
            // A UTF-16 file can grow when it becomes UTF-8.
            return TextRead::TooLarge;
        }
        TextRead::Text { text, lossy }
    }

    /// Reads at most `limit` bytes of a file.
    fn read_limited(&mut self, file: &SourceFile, limit: u64) -> Result<Vec<u8>, ApiError> {
        let mut bytes = Vec::new();
        let mut take = |reader: &mut dyn Read| -> io::Result<()> {
            reader.take(limit).read_to_end(&mut bytes).map(|_| ())
        };
        match &file.origin {
            Origin::Disk(path) => {
                let mut opened = File::open(path).map_err(|error| read_error(&error))?;
                take(&mut opened).map_err(|error| read_error(&error))?;
            }
            Origin::Zip { archive, index } => {
                let archive = self
                    .archives
                    .get_mut(*archive)
                    .ok_or_else(|| ApiError::internal("the archive is gone"))?;
                let mut entry = archive
                    .by_index(*index)
                    .map_err(|_| ApiError::storage("the archive entry could not be read"))?;
                take(&mut entry).map_err(|error| read_error(&error))?;
            }
        }
        Ok(bytes)
    }

    /// Opens a binary file of the source and gives `use_reader` a reader and the size the file
    /// system or the archive declares (the reader is not trusted to stop there: a caller that
    /// streams it counts). `NotFound` for a path the source does not list.
    ///
    /// # Errors
    ///
    /// What `use_reader` returns, `NotFound`, or `Storage` when the file cannot be opened.
    pub fn with_binary<T>(
        &mut self,
        path: &str,
        use_reader: impl FnOnce(&mut dyn Read, u64) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        let file = self
            .binary
            .get(path)
            .cloned()
            .ok_or_else(|| ApiError::not_found("the file is not in the source"))?;
        match file.origin {
            Origin::Disk(disk_path) => {
                let mut opened = File::open(&disk_path).map_err(|error| read_error(&error))?;
                let size = opened
                    .metadata()
                    .map_or(file.size, |metadata| metadata.len());
                use_reader(&mut opened, size)
            }
            Origin::Zip { archive, index } => {
                let archive = self
                    .archives
                    .get_mut(archive)
                    .ok_or_else(|| ApiError::internal("the archive is gone"))?;
                let mut entry = archive
                    .by_index(index)
                    .map_err(|_| ApiError::storage("the archive entry could not be read"))?;
                let size = entry.size();
                use_reader(&mut entry, size)
            }
        }
    }

    // --- internals ---

    /// Records a problem. `path` is shown to the person (control characters replaced, cut), never
    /// logged; `detail` is one of a few fixed words.
    fn warn(&mut self, path: &str, code: ImportWarningCode, detail: &str) {
        self.warnings.push(ImportWarning {
            path: display_path(path),
            code,
            detail: Some(detail.to_owned()).filter(|detail| !detail.is_empty()),
        });
    }

    fn warn_problem(&mut self, path: &str, problem: PathProblem) {
        let detail = match problem {
            PathProblem::Unsafe => "unsafe path",
            PathProblem::TooLong => "path too long",
        };
        self.warn(path, ImportWarningCode::UnsupportedFile, detail);
    }

    fn take_walk_problems(&mut self, walk: &Walk) {
        for (path, detail) in &walk.problems {
            self.warn(path, ImportWarningCode::UnsupportedFile, detail);
        }
    }

    /// Adds a file once: a path met twice (two parts of a split export) keeps the first.
    fn insert(&mut self, file: SourceFile, text: bool) {
        if !self.seen.insert(file.path.clone()) {
            self.warn(&file.path, ImportWarningCode::UnsupportedFile, "duplicate");
            return;
        }
        if text {
            self.text.push(file);
        } else if self.binary.len() >= MAX_BINARY_PATHS {
            if !self.binary_truncated {
                self.binary_truncated = true;
                self.warn("", ImportWarningCode::Truncated, "files");
            }
        } else {
            self.binary.insert(file.path.clone(), file);
        }
    }

    /// Sorts what was listed, so that the order does not depend on the file system.
    fn finish(&mut self) {
        self.text.sort_by(|a, b| a.path.cmp(&b.path));
    }

    /// The root of an AppFlowy backup in this source: the folder (`""`, or `<name>/` for one
    /// wrapping folder that holds every file) whose `relation_map.json` and `metadata.json` are
    /// both there.
    fn appflowy_root(&self) -> Option<String> {
        let has = |root: &str| {
            self.binary
                .contains_key(&format!("{root}{RELATION_MAP_NAME}"))
                && self.binary.contains_key(&format!("{root}{METADATA_NAME}"))
        };
        if has("") {
            return Some(String::new());
        }
        // One wrapping folder: every file of the source is inside the same top-level folder.
        let mut wrapper: Option<&str> = None;
        for path in self
            .binary
            .keys()
            .map(String::as_str)
            .chain(self.text.iter().map(|file| file.path.as_str()))
        {
            let (first, _) = path.split_once('/')?;
            match wrapper {
                None => wrapper = Some(first),
                Some(known) if known == first => {}
                Some(_) => return None,
            }
        }
        let root = format!("{}/", wrapper?);
        has(&root).then_some(root)
    }

    /// Reads an AppFlowy backup (see the module documentation): its `.json` files become text
    /// files and `collab_objects/` is dropped. A source that is not one is left as it is.
    fn adopt_appflowy_backup(&mut self) {
        let Some(root) = self.appflowy_root() else {
            return;
        };
        let skipped = format!("{root}{COLLAB_OBJECTS_DIR}/");
        let attached = format!("{root}{LOCAL_FILES_DIR}/");
        self.binary.retain(|path, _| !path.starts_with(&skipped));
        self.text.retain(|file| !file.path.starts_with(&skipped));
        let json_paths: Vec<String> = self
            .binary
            .keys()
            .filter(|path| {
                path.starts_with(&root)
                    && !path.starts_with(&attached)
                    && has_extension(path, "json")
            })
            .cloned()
            .collect();
        for path in json_paths {
            if let Some(file) = self.binary.remove(&path) {
                self.text.push(file);
            }
        }
        tracing::debug!(texts = self.text.len(), "an AppFlowy backup was recognised");
    }

    /// Lists the entries of `archive` and keeps the archive open for reading them. At the root of
    /// the outermost archive (`allow_nested`) a `.zip` entry is unpacked and merged in too.
    fn add_archive(&mut self, archive: ZipArchive<File>, allow_nested: bool, cache_dir: &Path) {
        let archive_index = self.archives.len();
        let mut nested: Vec<(usize, String, u64)> = Vec::new();
        for index in 0..archive.len() {
            if index >= ZIP_MAX_ENTRIES {
                self.warn("", ImportWarningCode::Truncated, "files");
                break;
            }
            let Ok(entry) = archive.by_index_data(index) else {
                self.warn("", ImportWarningCode::UnsupportedFile, "unreadable entry");
                continue;
            };
            let Ok(name) = entry.name().map(|name| name.into_owned()) else {
                self.warn("", ImportWarningCode::UnsupportedFile, "unreadable entry");
                continue;
            };
            if entry.is_dir() || name.ends_with('/') {
                continue;
            }
            let (size, link, encrypted) = (entry.size(), entry.is_symlink(), entry.encrypted());
            if let Some(problem) = archive_path::path_problem(&name) {
                self.warn_problem(&name, problem);
                continue;
            }
            if archive_path::is_skipped(&name) {
                continue;
            }
            if link {
                self.warn(&name, ImportWarningCode::UnsupportedFile, "link");
                continue;
            }
            if encrypted {
                self.warn(&name, ImportWarningCode::UnsupportedFile, "encrypted");
                continue;
            }
            if allow_nested && !name.contains('/') && has_extension(&name, "zip") {
                nested.push((index, name, size));
                continue;
            }
            let text = is_text_path(&name);
            let file = SourceFile {
                path: name,
                size,
                origin: Origin::Zip {
                    archive: archive_index,
                    index,
                },
            };
            self.insert(file, text);
        }
        self.archives.push(archive);
        for (index, name, size) in nested {
            self.add_nested(archive_index, index, name, size, cache_dir);
        }
    }

    /// Unpacks a zip found at the root of `outer` into a temporary file, opens it and merges its
    /// entries. A file that turns out not to be a zip is an ordinary binary file.
    fn add_nested(
        &mut self,
        outer: usize,
        index: usize,
        name: String,
        size: u64,
        cache_dir: &Path,
    ) {
        if size > NESTED_ZIP_MAX_BYTES {
            self.warn(&name, ImportWarningCode::TooLarge, "");
            return;
        }
        let Ok(path) = self.unpack_nested(outer, index, cache_dir) else {
            self.warn(&name, ImportWarningCode::UnsupportedFile, "nested zip");
            return;
        };
        let opened = File::open(&path)
            .map_err(|error| read_error(&error))
            .and_then(|file| {
                ZipArchive::new(file)
                    .map_err(|_| ApiError::invalid("path", "the file is not a valid zip archive"))
            });
        match opened {
            Ok(inner) => self.add_archive(inner, false, cache_dir),
            Err(error) if error.code == kb_types::ErrorCode::InvalidInput => {
                // Not a zip after all (or damaged): an attachment like any other file, and the
                // copy that was made to find out is not needed any more.
                self.temp_files.retain(|temp| temp != &path);
                let _ = fs::remove_file(&path);
                let file = SourceFile {
                    path: name,
                    size,
                    origin: Origin::Zip {
                        archive: outer,
                        index,
                    },
                };
                self.insert(file, false);
            }
            Err(_) => self.warn(&name, ImportWarningCode::UnsupportedFile, "nested zip"),
        }
    }

    /// Copies a nested zip out of its archive into a fresh file under `<cache_dir>/import`, at most
    /// [`NESTED_ZIP_MAX_BYTES`] bytes. The file is registered for removal before it is written.
    fn unpack_nested(
        &mut self,
        outer: usize,
        index: usize,
        cache_dir: &Path,
    ) -> Result<PathBuf, ApiError> {
        let dir = cache_dir.join(IMPORT_DIR);
        fs::create_dir_all(&dir).map_err(|error| temp_error(&error))?;
        let (path, mut file) = create_unique_file(&dir).map_err(|error| temp_error(&error))?;
        self.temp_files.push(path.clone());
        let archive = self
            .archives
            .get_mut(outer)
            .ok_or_else(|| ApiError::internal("the archive is gone"))?;
        let mut entry = archive
            .by_index(index)
            .map_err(|_| ApiError::storage("the archive entry could not be read"))?;
        let copied = io::copy(&mut (&mut entry).take(NESTED_ZIP_MAX_BYTES + 1), &mut file)
            .map_err(|error| temp_error(&error))?;
        file.flush().map_err(|error| temp_error(&error))?;
        if copied > NESTED_ZIP_MAX_BYTES {
            return Err(ApiError::storage("the nested zip is too large"));
        }
        Ok(path)
    }
}

/// Removes every temporary file of nested zips: the whole `import` folder of the app cache
/// folder. Run when the app starts; a folder that is not there is fine, and a failure (a file
/// still open in another instance, say) is logged and does not stop the start.
pub fn clear_import_dir(cache_dir: &Path) {
    match fs::remove_dir_all(cache_dir.join(IMPORT_DIR)) {
        Ok(()) => tracing::debug!("temporary import files removed"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(%error, "temporary import files could not all be removed");
        }
    }
}

/// A new, empty file in `dir` with a name made here (never one the source chose), private to the
/// owner on Unix.
fn create_unique_file(dir: &Path) -> io::Result<(PathBuf, File)> {
    let process = std::process::id();
    for _ in 0..100 {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos());
        let path = dir.join(format!("part-{process}-{nanos}-{counter}.zip"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other("no free temporary file name"))
}

fn temp_error(error: &io::Error) -> ApiError {
    ApiError::storage(format!("a temporary file could not be written: {error}"))
}

fn read_error(error: &io::Error) -> ApiError {
    ApiError::storage(format!("the file could not be read: {error}"))
}

// --- walking a folder ---

/// A file found by [`walk_tree`].
struct Found {
    relative: String,
    path: PathBuf,
    size: u64,
}

impl Found {
    fn into_file(self) -> SourceFile {
        SourceFile {
            path: self.relative,
            size: self.size,
            origin: Origin::Disk(self.path),
        }
    }
}

/// What a walk found.
#[derive(Default)]
struct Walk {
    files: Vec<Found>,
    /// The walk stopped at its file limit.
    truncated: bool,
    /// Entries left out because of their name: shown path and fixed detail.
    problems: Vec<(String, &'static str)>,
}

/// Lists the files under `root` breadth first (so a limit leaves out the deepest), each folder's
/// entries in name order. Hidden entries, `__MACOSX`, `node_modules`, links and anything that is
/// not a regular file or a folder are skipped; a name that is not valid UTF-8 or not a safe path
/// is a problem; a folder that cannot be read is left out. Files at depth `max_depth` or less
/// (a file directly in `root` has depth 1) are listed, at most `max_files` of them.
fn walk_tree(root: &Path, max_depth: usize, max_files: usize) -> Walk {
    let mut walk = Walk::default();
    let mut queue: VecDeque<(PathBuf, String, usize)> = VecDeque::new();
    queue.push_back((root.to_path_buf(), String::new(), 0));
    while let Some((dir, prefix, depth)) = queue.pop_front() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<fs::DirEntry> = read.filter_map(Result::ok).collect();
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                walk.problems
                    .push((file_name.to_string_lossy().into_owned(), "name"));
                continue;
            };
            if archive_path::is_skipped(name) {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            let relative = format!("{prefix}{name}");
            if kind.is_dir() {
                if depth + 1 < max_depth {
                    queue.push_back((entry.path(), format!("{relative}/"), depth + 1));
                }
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            if let Some(problem) = archive_path::path_problem(&relative) {
                let detail = match problem {
                    PathProblem::Unsafe => "unsafe path",
                    PathProblem::TooLong => "path too long",
                };
                walk.problems.push((relative, detail));
                continue;
            }
            if walk.files.len() >= max_files {
                walk.truncated = true;
                return walk;
            }
            let size = entry.metadata().map_or(0, |metadata| metadata.len());
            walk.files.push(Found {
                relative,
                path: entry.path(),
                size,
            });
        }
    }
    walk
}

// --- paths ---

/// The folder every file is in, or `None` when there is none (files on different drives).
fn common_folder(files: &[PathBuf]) -> Option<PathBuf> {
    let mut common: Option<Vec<Component<'_>>> = None;
    for file in files {
        let parts: Vec<Component<'_>> = file.parent()?.components().collect();
        common = Some(match common {
            None => parts,
            Some(previous) => previous
                .iter()
                .zip(parts.iter())
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| *a)
                .collect(),
        });
    }
    let parts = common?;
    // Files on different drives share nothing (a drive's root is a folder like any other).
    if parts.is_empty() {
        return None;
    }
    Some(parts.iter().collect())
}

/// `file` relative to `root` as `/`-joined text; `None` when it is not under it or a segment is
/// not valid UTF-8.
fn relative_text(root: &Path, file: &Path) -> Option<String> {
    let relative = file.strip_prefix(root).ok()?;
    let mut segments = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(segment) => segments.push(segment.to_str()?),
            _ => return None,
        }
    }
    (!segments.is_empty()).then(|| segments.join("/"))
}

/// How two paths of the same folder are compared: Windows and macOS ignore case.
fn path_key(path: &str) -> String {
    if cfg!(any(windows, target_os = "macos")) {
        path.to_lowercase()
    } else {
        path.to_owned()
    }
}

fn has_extension(name: &str, extension: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, found)| found.eq_ignore_ascii_case(extension))
}

fn lossy(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// A path as a warning shows it: control characters replaced, at most 300 characters.
fn display_path(path: &str) -> String {
    path.chars()
        .take(WARNING_PATH_CHARS)
        .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
        .collect()
}

// --- decoding ---

/// Decodes the bytes of a text file: UTF-8 (a byte order mark removed), UTF-16 little or big endian
/// when it starts with the matching byte order mark, anything else as UTF-8 with replacement
/// characters. The flag is `true` when replacement characters were needed.
#[must_use]
pub fn decode_text(bytes: &[u8]) -> (String, bool) {
    if let Some(body) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(body, true);
    }
    if let Some(body) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(body, false);
    }
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(body) {
        Ok(text) => (text.to_owned(), false),
        Err(_) => (String::from_utf8_lossy(body).into_owned(), true),
    }
}

fn decode_utf16(body: &[u8], little_endian: bool) -> (String, bool) {
    let (pairs, rest) = body.as_chunks::<2>();
    let units = pairs.iter().map(|&pair| {
        if little_endian {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        }
    });
    let mut text = String::with_capacity(body.len() / 2);
    let mut lossy = false;
    for decoded in char::decode_utf16(units) {
        match decoded {
            Ok(c) => text.push(c),
            Err(_) => {
                text.push('\u{FFFD}');
                lossy = true;
            }
        }
    }
    // A trailing byte is half a unit.
    if !rest.is_empty() {
        text.push('\u{FFFD}');
        lossy = true;
    }
    (text, lossy)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn text_files_are_told_by_their_extension_and_the_sidecar_by_its_name() {
        for path in [
            "a.md",
            "A.MD",
            "dir/Note.markdown",
            "x.txt",
            "p.html",
            "p.HTM",
            "t.csv",
            "board.excalidraw",
            "atomik-export.json",
            "a.b.c.md",
        ] {
            assert!(is_text_path(path), "{path}");
        }
        for path in [
            "a.png",
            "a.pdf",
            "md",
            "noextension",
            "dir.md/file",
            "other.json",
            "dir/atomik-export.json",
            "a.md.png",
        ] {
            assert!(!is_text_path(path), "{path}");
        }
    }

    #[test]
    fn utf8_is_decoded_and_its_byte_order_mark_removed() {
        assert_eq!(decode_text("héllo".as_bytes()), ("héllo".to_owned(), false));
        let mut with_mark = vec![0xEF, 0xBB, 0xBF];
        with_mark.extend_from_slice("日本".as_bytes());
        assert_eq!(decode_text(&with_mark), ("日本".to_owned(), false));
        assert_eq!(decode_text(b""), (String::new(), false));
    }

    #[test]
    fn utf16_with_a_byte_order_mark_is_decoded_either_way_round() {
        let text = "Grüße 😀";
        let mut little = vec![0xFF, 0xFE];
        let mut big = vec![0xFE, 0xFF];
        for unit in text.encode_utf16() {
            little.extend_from_slice(&unit.to_le_bytes());
            big.extend_from_slice(&unit.to_be_bytes());
        }
        assert_eq!(decode_text(&little), (text.to_owned(), false));
        assert_eq!(decode_text(&big), (text.to_owned(), false));
    }

    #[test]
    fn broken_utf16_is_replaced_and_reported() {
        // A lone high surrogate, then a stray byte.
        let broken = [0xFF, 0xFE, 0x41, 0x00, 0x00, 0xD8, 0x42];
        let (text, lossy) = decode_text(&broken);
        assert_eq!(text, "A\u{FFFD}\u{FFFD}");
        assert!(lossy);
    }

    #[test]
    fn anything_else_is_utf8_with_replacement_characters_and_a_flag() {
        let (text, lossy) = decode_text(&[b'a', 0xE9, b'b']);
        assert_eq!(text, "a\u{FFFD}b");
        assert!(lossy);
        // Latin-1 is not guessed.
        let (text, lossy) = decode_text(b"caf\xE9");
        assert_eq!(text, "caf\u{FFFD}");
        assert!(lossy);
    }

    #[test]
    fn warnings_show_a_cleaned_and_cut_path() {
        let shown = display_path(&format!("a\u{7}b{}", "x".repeat(500)));
        assert!(shown.starts_with("a\u{FFFD}b"));
        assert_eq!(shown.chars().count(), WARNING_PATH_CHARS);
    }

    #[test]
    fn the_common_folder_of_picked_files_is_their_shared_parent() {
        let base = std::env::temp_dir().join("kb-common-folder");
        let files = [base.join("a.md"), base.join("b.md")];
        assert_eq!(common_folder(&files), Some(base.clone()));
        let nested = [base.join("a.md"), base.join("sub").join("b.md")];
        assert_eq!(common_folder(&nested), Some(base.clone()));
        assert_eq!(common_folder(&[]), None);
        // Files in the root of a drive are picked from a folder too.
        let root = base.ancestors().last().unwrap().to_path_buf();
        assert_eq!(common_folder(&[root.join("a.md")]), Some(root));
        // A relative name has no folder to speak of.
        assert_eq!(common_folder(&[PathBuf::from("a.md")]), None);
        assert_eq!(
            relative_text(&base, &base.join("sub").join("b.md")),
            Some("sub/b.md".to_owned())
        );
        assert_eq!(relative_text(&base, &base), None);
    }
}
