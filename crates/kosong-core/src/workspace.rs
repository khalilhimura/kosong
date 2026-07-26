//! Workspace discovery, containment, and durable writes.
//!
//! A *workspace* is the directory holding the user's document. It is also the
//! OKF bundle root, so anything written here is potentially visible to other
//! OKF tooling.
//!
//! Every write in `kosong` goes through [`atomic_write`]. The product promise
//! is that a beginner cannot lose work by running a command, which means a
//! crash or a full disk must never leave a half-written document behind. The
//! sequence is: write a temporary sibling, `fsync` it, rename over the target,
//! then `fsync` the directory so the rename itself is durable.
//!
//! The containment rules exist because the document path can come from a
//! `--path` flag. Refusing symlinks and refusing traversal outside the
//! canonicalized root keeps a mistyped or hostile path from writing somewhere
//! the user did not intend.

use crate::okf::{self, Document, OkfError};
use camino::{Utf8Path, Utf8PathBuf};
use std::fs::{self, File};
use std::io::{self, Write};

/// Largest document `kosong` will read or write, in bytes.
///
/// The same limit is enforced by the API, per the technical specification.
/// One mebibyte is far beyond a single page and still small enough to keep
/// sync predictable.
pub const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;

/// Directory holding per-project, non-secret state.
pub const STATE_DIR: &str = ".kosong";

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("no kosong document found here or in any parent folder")]
    NotFound,

    #[error("`{path}` is not a folder")]
    NotADirectory { path: Utf8PathBuf },

    #[error("`{path}` is a shortcut to somewhere else")]
    IsSymlink { path: Utf8PathBuf },

    #[error("`{path}` is outside the workspace folder")]
    OutsideWorkspace { path: Utf8PathBuf },

    #[error("`{path}` is not a valid UTF-8 path")]
    NonUtf8Path { path: std::path::PathBuf },

    #[error("`{name}` is reserved by the Open Knowledge Format")]
    ReservedFilename { name: String },

    #[error("this document is {size} bytes, larger than the {MAX_DOCUMENT_BYTES} byte limit")]
    TooLarge { size: usize },

    #[error("a document already exists at `{path}`")]
    AlreadyExists { path: Utf8PathBuf },

    #[error("could not read or write `{path}`: {source}")]
    Io {
        path: Utf8PathBuf,
        #[source]
        source: io::Error,
    },

    #[error(transparent)]
    Okf(#[from] OkfError),
}

impl WorkspaceError {
    /// A plain-language next action, in the CLI's voice.
    pub fn repair(&self) -> String {
        match self {
            Self::NotFound => "Run: kosong start".into(),
            Self::NotADirectory { .. } => "Point --path at a folder, not a file.".into(),
            Self::IsSymlink { path } => format!(
                "kosong will not write through a shortcut, because it is not obvious where the file really goes. Replace `{path}` with a real file, or choose another location."
            ),
            Self::OutsideWorkspace { .. } => {
                "Choose a location inside your workspace folder.".into()
            }
            Self::NonUtf8Path { .. } => {
                "Rename the folder so its name uses ordinary characters.".into()
            }
            Self::ReservedFilename { .. } => format!(
                "The Open Knowledge Format uses index.md and log.md for other purposes. Try {}.",
                okf::DEFAULT_FILENAME
            ),
            Self::TooLarge { .. } => {
                "Shorten the document, or split it into a separate file.".into()
            }
            Self::AlreadyExists { .. } => {
                "Run `kosong show` to see it, or choose another name with --path.".into()
            }
            Self::Io { .. } => "Check the folder exists and that you can write to it.".into(),
            Self::Okf(e) => e.repair(),
        }
    }
}

/// Converts a `std` path into a UTF-8 path, or reports it.
fn to_utf8(path: std::path::PathBuf) -> Result<Utf8PathBuf, WorkspaceError> {
    Utf8PathBuf::from_path_buf(path).map_err(|p| WorkspaceError::NonUtf8Path { path: p })
}

fn io_err(path: &Utf8Path, source: io::Error) -> WorkspaceError {
    WorkspaceError::Io {
        path: path.to_owned(),
        source,
    }
}

// ---------------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------------

/// A directory containing a `kosong` document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    root: Utf8PathBuf,
    document: Utf8PathBuf,
}

impl Workspace {
    /// Opens `root` as a workspace using the default document filename.
    ///
    /// The root is canonicalized immediately, so every later containment check
    /// compares against a real, fully resolved path.
    pub fn open(root: &Utf8Path) -> Result<Self, WorkspaceError> {
        Self::open_with_document(root, okf::DEFAULT_FILENAME)
    }

    /// Opens `root` as a workspace holding the named document.
    pub fn open_with_document(root: &Utf8Path, filename: &str) -> Result<Self, WorkspaceError> {
        if okf::is_reserved_filename(filename) {
            return Err(WorkspaceError::ReservedFilename {
                name: filename.to_owned(),
            });
        }

        let canonical = to_utf8(fs::canonicalize(root).map_err(|e| io_err(root, e))?)?;
        if !canonical.is_dir() {
            return Err(WorkspaceError::NotADirectory { path: canonical });
        }

        let document = canonical.join(filename);
        Ok(Self {
            root: canonical,
            document,
        })
    }

    /// Walks up from `start` looking for a workspace.
    ///
    /// A directory qualifies if it holds the default document or a `.kosong`
    /// state directory, which mirrors how `git` finds its root and means the
    /// user can run commands from a subfolder.
    pub fn discover(start: &Utf8Path) -> Result<Self, WorkspaceError> {
        let canonical = to_utf8(fs::canonicalize(start).map_err(|e| io_err(start, e))?)?;

        for dir in canonical.ancestors() {
            if dir.join(okf::DEFAULT_FILENAME).is_file() || dir.join(STATE_DIR).is_dir() {
                return Self::open(dir);
            }
        }
        Err(WorkspaceError::NotFound)
    }

    pub fn root(&self) -> &Utf8Path {
        &self.root
    }

    pub fn document_path(&self) -> &Utf8Path {
        &self.document
    }

    /// The per-project state directory, `<root>/.kosong`.
    pub fn state_dir(&self) -> Utf8PathBuf {
        self.root.join(STATE_DIR)
    }

    /// Whether a document exists on disk yet.
    pub fn has_document(&self) -> bool {
        self.document.is_file()
    }

    /// Resolves `relative` inside the workspace, refusing anything that escapes it.
    ///
    /// Traversal is checked lexically after normalization rather than by
    /// canonicalizing, because the target may not exist yet.
    pub fn resolve(&self, relative: &str) -> Result<Utf8PathBuf, WorkspaceError> {
        let candidate = self.root.join(relative);
        let normalized = normalize(&candidate);

        if !normalized.starts_with(&self.root) {
            return Err(WorkspaceError::OutsideWorkspace { path: normalized });
        }
        Ok(normalized)
    }

    /// Reads and parses the document.
    pub fn read_document(&self) -> Result<Document, WorkspaceError> {
        let text = read_checked(&self.document)?;
        Ok(Document::parse(&text)?)
    }

    /// Writes the document atomically.
    pub fn write_document(&self, document: &Document) -> Result<(), WorkspaceError> {
        let text = document.to_okf_string()?;
        if text.len() > MAX_DOCUMENT_BYTES {
            return Err(WorkspaceError::TooLarge { size: text.len() });
        }
        refuse_symlink(&self.document)?;
        atomic_write(&self.document, text.as_bytes())
    }

    /// Writes a new document, refusing to clobber an existing one.
    pub fn create_document(&self, document: &Document) -> Result<(), WorkspaceError> {
        if self.document.exists() {
            return Err(WorkspaceError::AlreadyExists {
                path: self.document.clone(),
            });
        }
        self.write_document(document)
    }
}

// ---------------------------------------------------------------------------
// Filesystem primitives
// ---------------------------------------------------------------------------

/// Rejects a path that is a symlink.
///
/// Uses `symlink_metadata`, which does not follow the link — the whole point
/// is to detect the link itself. A missing path is fine; it is not a symlink.
pub fn refuse_symlink(path: &Utf8Path) -> Result<(), WorkspaceError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(WorkspaceError::IsSymlink {
            path: path.to_owned(),
        }),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_err(path, e)),
    }
}

/// Reads a file, enforcing the size limit and refusing symlinks.
pub fn read_checked(path: &Utf8Path) -> Result<String, WorkspaceError> {
    refuse_symlink(path)?;

    let meta = fs::metadata(path).map_err(|e| io_err(path, e))?;
    if meta.len() as usize > MAX_DOCUMENT_BYTES {
        return Err(WorkspaceError::TooLarge {
            size: meta.len() as usize,
        });
    }
    fs::read_to_string(path).map_err(|e| io_err(path, e))
}

/// Writes `contents` to `path` durably and atomically.
///
/// A reader of `path` sees either the old bytes or the new bytes, never a
/// mixture, even if the process dies mid-write.
pub fn atomic_write(path: &Utf8Path, contents: &[u8]) -> Result<(), WorkspaceError> {
    write_inner(path, contents, None)
}

/// Like [`atomic_write`], but creates the file readable only by its owner.
///
/// For any credential written to disk as a fallback when the OS keychain is
/// unavailable.
pub fn atomic_write_private(path: &Utf8Path, contents: &[u8]) -> Result<(), WorkspaceError> {
    write_inner(path, contents, Some(0o600))
}

fn write_inner(path: &Utf8Path, contents: &[u8], mode: Option<u32>) -> Result<(), WorkspaceError> {
    let parent = path.parent().unwrap_or(Utf8Path::new("."));
    fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;

    // A sibling temp file guarantees the rename stays on one filesystem, which
    // is what makes it atomic. Including the process id avoids two concurrent
    // kosong processes colliding.
    let filename = path.file_name().unwrap_or("document");
    let temp = parent.join(format!(".{filename}.{}.tmp", std::process::id()));

    // Any early return past this point must not leave the temp file behind.
    let result = (|| -> Result<(), WorkspaceError> {
        let mut file = File::create(&temp).map_err(|e| io_err(&temp, e))?;

        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(mode))
                .map_err(|e| io_err(&temp, e))?;
        }
        #[cfg(not(unix))]
        let _ = mode;

        file.write_all(contents).map_err(|e| io_err(&temp, e))?;
        // Flush the bytes before the rename, or the rename may be durable
        // while the contents are not.
        file.sync_all().map_err(|e| io_err(&temp, e))?;
        drop(file);

        fs::rename(&temp, path).map_err(|e| io_err(path, e))?;

        // Persist the directory entry itself. Without this, the rename can be
        // lost on power failure even though the file contents survived.
        sync_dir(parent);
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Best-effort `fsync` of a directory.
///
/// Not all platforms support opening a directory for sync, and failing to do
/// so does not make the write incorrect, only less durable. So this never
/// turns a successful write into an error.
fn sync_dir(dir: &Utf8Path) {
    if let Ok(handle) = File::open(dir) {
        let _ = handle.sync_all();
    }
}

/// Lexically resolves `.` and `..` without touching the filesystem.
///
/// Used for containment checks on paths that do not exist yet, where
/// `canonicalize` would fail.
fn normalize(path: &Utf8Path) -> Utf8PathBuf {
    let mut out = Utf8PathBuf::new();
    for component in path.components() {
        match component {
            camino::Utf8Component::CurDir => {}
            camino::Utf8Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_str()),
        }
    }
    out
}
