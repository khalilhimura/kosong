//! Filesystem safety and durability guarantees.
//!
//! Covers the rules in technical specification §6.4: canonicalized root,
//! symlink refusal, traversal refusal, and atomic writes.

use camino::{Utf8Path, Utf8PathBuf};
use kosong_core::okf::Document;
use kosong_core::workspace::{self, MAX_DOCUMENT_BYTES, Workspace, WorkspaceError};
use tempfile::TempDir;

/// A temporary directory as a UTF-8 path.
///
/// The path is canonicalized because macOS reports `/var` while the real path
/// is `/private/var`; without this, containment comparisons against a
/// canonicalized workspace root would spuriously fail.
fn temp() -> (TempDir, Utf8PathBuf) {
    let dir = TempDir::new().expect("temp dir");
    let path = std::fs::canonicalize(dir.path()).expect("canonicalize");
    let utf8 = Utf8PathBuf::from_path_buf(path).expect("utf-8 temp path");
    (dir, utf8)
}

// ---------------------------------------------------------------------------
// Atomic writes
// ---------------------------------------------------------------------------

#[test]
fn atomic_write_creates_the_file() {
    let (_guard, root) = temp();
    let target = root.join("out.txt");

    workspace::atomic_write(&target, b"hello").unwrap();

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
}

#[test]
fn atomic_write_replaces_existing_content_entirely() {
    let (_guard, root) = temp();
    let target = root.join("out.txt");

    workspace::atomic_write(&target, b"a much longer original value").unwrap();
    workspace::atomic_write(&target, b"short").unwrap();

    // A non-atomic implementation that truncates and rewrites could leave
    // trailing bytes from the longer value.
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "short");
}

#[test]
fn atomic_write_leaves_no_temporary_file_behind() {
    let (_guard, root) = temp();
    workspace::atomic_write(&root.join("out.txt"), b"hello").unwrap();

    let leftovers: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != "out.txt")
        .collect();

    assert!(
        leftovers.is_empty(),
        "temporary files were left behind: {leftovers:?}"
    );
}

#[test]
fn atomic_write_creates_missing_parent_directories() {
    let (_guard, root) = temp();
    let target = root.join("deeply").join("nested").join("out.txt");

    workspace::atomic_write(&target, b"hello").unwrap();

    assert!(target.is_file());
}

#[cfg(unix)]
#[test]
fn private_writes_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let (_guard, root) = temp();
    let target = root.join("credential");
    workspace::atomic_write_private(&target, b"secret").unwrap();

    let mode = std::fs::metadata(&target).unwrap().permissions().mode();
    assert_eq!(
        mode & 0o777,
        0o600,
        "a credential fallback file must not be readable by other users"
    );
}

// ---------------------------------------------------------------------------
// Symlink refusal
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn a_symlinked_document_is_refused() {
    let (_guard, root) = temp();
    let real = root.join("real.md");
    std::fs::write(&real, "---\ntype: Page\n---\nbody\n").unwrap();

    let link = root.join("kosong.md");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let workspace = Workspace::open(&root).unwrap();
    let err = workspace.read_document().unwrap_err();

    assert!(matches!(err, WorkspaceError::IsSymlink { .. }));
    assert!(err.repair().contains("shortcut"));
}

#[cfg(unix)]
#[test]
fn writing_through_a_symlink_is_refused() {
    let (_guard, root) = temp();
    let outside_dir = TempDir::new().unwrap();
    let outside = Utf8PathBuf::from_path_buf(std::fs::canonicalize(outside_dir.path()).unwrap())
        .unwrap()
        .join("victim.md");
    std::fs::write(&outside, "original").unwrap();

    std::os::unix::fs::symlink(&outside, root.join("kosong.md")).unwrap();

    let workspace = Workspace::open(&root).unwrap();
    let document = Document::new("Attacker").unwrap();
    let err = workspace.write_document(&document).unwrap_err();

    assert!(matches!(err, WorkspaceError::IsSymlink { .. }));
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "original",
        "the file the symlink pointed at must be untouched"
    );
}

#[test]
fn a_plain_file_is_not_mistaken_for_a_symlink() {
    let (_guard, root) = temp();
    let target = root.join("kosong.md");
    std::fs::write(&target, "x").unwrap();

    assert!(workspace::refuse_symlink(&target).is_ok());
}

#[test]
fn a_missing_path_is_not_a_symlink() {
    let (_guard, root) = temp();
    assert!(workspace::refuse_symlink(&root.join("nothing-here.md")).is_ok());
}

// ---------------------------------------------------------------------------
// Containment
// ---------------------------------------------------------------------------

#[test]
fn paths_inside_the_workspace_resolve() {
    let (_guard, root) = temp();
    let workspace = Workspace::open(&root).unwrap();

    assert_eq!(
        workspace.resolve("kosong.md").unwrap(),
        workspace.root().join("kosong.md")
    );
    assert_eq!(
        workspace.resolve("nested/deep/file.md").unwrap(),
        workspace.root().join("nested").join("deep").join("file.md")
    );
    // Traversal that stays inside is fine.
    assert!(workspace.resolve("nested/../kosong.md").is_ok());
}

#[test]
fn traversal_outside_the_workspace_is_refused() {
    let (_guard, root) = temp();
    let workspace = Workspace::open(&root).unwrap();

    for escape in [
        "../escaped.md",
        "../../escaped.md",
        "nested/../../escaped.md",
        "./../../etc/passwd",
    ] {
        let result = workspace.resolve(escape);
        assert!(
            matches!(result, Err(WorkspaceError::OutsideWorkspace { .. })),
            "`{escape}` should have been refused, got {result:?}"
        );
    }
}

#[test]
fn an_absolute_path_outside_the_workspace_is_refused() {
    let (_guard, root) = temp();
    let workspace = Workspace::open(&root).unwrap();

    assert!(matches!(
        workspace.resolve("/etc/passwd"),
        Err(WorkspaceError::OutsideWorkspace { .. })
    ));
}

// ---------------------------------------------------------------------------
// Reserved filenames
// ---------------------------------------------------------------------------

#[test]
fn reserved_filenames_cannot_be_opened_as_documents() {
    let (_guard, root) = temp();

    for reserved in ["index.md", "log.md", "INDEX.md"] {
        let result = Workspace::open_with_document(&root, reserved);
        assert!(
            matches!(result, Err(WorkspaceError::ReservedFilename { .. })),
            "`{reserved}` must be refused"
        );
    }
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

#[test]
fn discovery_walks_up_to_the_workspace_root() {
    let (_guard, root) = temp();
    std::fs::write(root.join("kosong.md"), "---\ntype: Page\n---\n").unwrap();

    let nested = root.join("a").join("b").join("c");
    std::fs::create_dir_all(&nested).unwrap();

    let found = Workspace::discover(&nested).unwrap();
    assert_eq!(found.root(), root.as_path());
}

#[test]
fn discovery_recognises_a_state_directory() {
    let (_guard, root) = temp();
    std::fs::create_dir_all(root.join(".kosong")).unwrap();

    let found = Workspace::discover(&root).unwrap();
    assert_eq!(found.root(), root.as_path());
}

#[test]
fn discovery_reports_a_useful_error_when_there_is_nothing() {
    let (_guard, root) = temp();

    let err = Workspace::discover(&root).unwrap_err();
    assert!(matches!(err, WorkspaceError::NotFound));
    assert_eq!(err.repair(), "Run: kosong start");
}

// ---------------------------------------------------------------------------
// Document round trip
// ---------------------------------------------------------------------------

#[test]
fn a_document_survives_a_write_and_read_cycle() {
    let (_guard, root) = temp();
    let workspace = Workspace::open(&root).unwrap();

    let original = Document::new("My First Site").unwrap();
    workspace.create_document(&original).unwrap();

    let loaded = workspace.read_document().unwrap();
    assert_eq!(loaded.title(), Some("My First Site"));
    assert_eq!(loaded.body(), original.body());
    assert_eq!(
        loaded.kosong().unwrap().unwrap().id,
        original.kosong().unwrap().unwrap().id
    );
}

#[test]
fn creating_a_document_twice_is_refused() {
    let (_guard, root) = temp();
    let workspace = Workspace::open(&root).unwrap();
    let document = Document::new("First").unwrap();

    workspace.create_document(&document).unwrap();
    let err = workspace
        .create_document(&Document::new("Second").unwrap())
        .unwrap_err();

    assert!(matches!(err, WorkspaceError::AlreadyExists { .. }));
    // The original must survive the refusal.
    assert_eq!(workspace.read_document().unwrap().title(), Some("First"));
}

#[test]
fn an_oversized_document_is_refused_on_read() {
    let (_guard, root) = temp();
    let path = root.join("kosong.md");

    let huge = format!(
        "---\ntype: Page\n---\n{}",
        "x".repeat(MAX_DOCUMENT_BYTES + 1)
    );
    std::fs::write(&path, huge).unwrap();

    let err = Workspace::open(&root).unwrap().read_document().unwrap_err();
    assert!(matches!(err, WorkspaceError::TooLarge { .. }));
}

#[test]
fn has_document_reports_presence() {
    let (_guard, root) = temp();
    let workspace = Workspace::open(&root).unwrap();

    assert!(!workspace.has_document());
    workspace
        .create_document(&Document::new("Here").unwrap())
        .unwrap();
    assert!(workspace.has_document());
}

#[test]
fn the_state_directory_sits_inside_the_workspace() {
    let (_guard, root) = temp();
    let workspace = Workspace::open(&root).unwrap();

    assert_eq!(workspace.state_dir(), workspace.root().join(".kosong"));
}

#[test]
fn opening_a_file_as_a_workspace_is_refused() {
    let (_guard, root) = temp();
    let file = root.join("not-a-dir");
    std::fs::write(&file, "x").unwrap();

    assert!(matches!(
        Workspace::open(&file),
        Err(WorkspaceError::NotADirectory { .. })
    ));
}

#[test]
fn opening_a_missing_directory_reports_io_not_panic() {
    let (_guard, root) = temp();
    let missing: &Utf8Path = &root.join("does-not-exist");

    assert!(matches!(
        Workspace::open(missing),
        Err(WorkspaceError::Io { .. })
    ));
}
