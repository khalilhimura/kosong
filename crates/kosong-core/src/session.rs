//! Where the refresh token lives.
//!
//! Only the **refresh token** is ever stored. §8.2 is explicit: "CLI writes
//! only the refresh token to OS keychain." Access tokens are short-lived and
//! obtained by refreshing, so they stay in memory for the life of one command
//! and are never written anywhere.
//!
//! # Storage backends
//!
//! The operating-system keychain is used where available. §6.4 permits a file
//! fallback, but requires restrictive permissions and a prominent warning —
//! both are implemented here, and [`SessionStore::backend`] reports which is
//! in use so `doctor` and `status` can say so plainly rather than letting a
//! user assume their credential is in the keychain when it is not.

use crate::workspace::{self, WorkspaceError};
use camino::{Utf8Path, Utf8PathBuf};

/// Service name under which the credential is filed in the OS keychain.
const KEYCHAIN_SERVICE: &str = "dev.kosong.cli";

/// Account name within the service. One session per machine in v1.
const KEYCHAIN_ACCOUNT: &str = "refresh-token";

/// Filename used when no keychain is available.
const FALLBACK_FILENAME: &str = "session";

/// Environment variable forcing the credential into a specific file.
///
/// For tests and CI, which must never touch a developer's real keychain.
pub const SESSION_FILE_ENV: &str = "KOSONG_SESSION_FILE";

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("could not reach the system keychain: {0}")]
    Keychain(String),

    #[error(transparent)]
    Storage(#[from] WorkspaceError),

    #[error("could not work out where to keep your sign-in: {0}")]
    NoLocation(String),
}

impl SessionError {
    pub fn repair(&self) -> String {
        match self {
            Self::Keychain(_) => {
                "Your system keychain could not be reached. Sign in again with `kosong login`, \
                 and kosong will keep your sign-in in a protected file instead."
                    .into()
            }
            Self::Storage(e) => e.repair(),
            Self::NoLocation(_) => "Set KOSONG_CONFIG_DIR to a folder kosong may use.".into(),
        }
    }
}

/// Which store holds the credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// The operating-system keychain. Preferred.
    Keychain,
    /// A file readable only by the owner. Used when no keychain is available.
    File,
    /// Memory only. Tests.
    Memory,
}

impl Backend {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Keychain => "system keychain",
            Self::File => "a protected file",
            Self::Memory => "memory (test only)",
        }
    }

    /// Whether the user should be told about this choice.
    ///
    /// A file fallback is a real reduction in protection, and §6.4 requires
    /// saying so prominently rather than quietly degrading.
    pub fn warrants_warning(self) -> bool {
        matches!(self, Self::File)
    }
}

/// Reads and writes the stored refresh token.
pub trait SessionStore {
    fn save(&self, refresh_token: &str) -> Result<(), SessionError>;
    fn load(&self) -> Result<Option<String>, SessionError>;
    fn clear(&self) -> Result<(), SessionError>;
    fn backend(&self) -> Backend;
}

// ---------------------------------------------------------------------------
// Keychain
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod os_keychain {
    use super::{Backend, KEYCHAIN_ACCOUNT, KEYCHAIN_SERVICE, SessionError, SessionStore};

    pub struct KeychainStore;

    impl KeychainStore {
        /// Returns a store only if the keychain actually works.
        ///
        /// Probing with a read rather than assuming success: a Linux box with
        /// no Secret Service running still compiles the backend in, and
        /// discovering that at sign-in time is better than at read time.
        pub fn available() -> Option<Self> {
            let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).ok()?;
            match entry.get_password() {
                Ok(_) => Some(Self),
                Err(keyring::Error::NoEntry) => Some(Self),
                Err(_) => None,
            }
        }

        fn entry() -> Result<keyring::Entry, SessionError> {
            keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
                .map_err(|e| SessionError::Keychain(e.to_string()))
        }
    }

    impl SessionStore for KeychainStore {
        fn save(&self, refresh_token: &str) -> Result<(), SessionError> {
            Self::entry()?
                .set_password(refresh_token)
                .map_err(|e| SessionError::Keychain(e.to_string()))
        }

        fn load(&self) -> Result<Option<String>, SessionError> {
            match Self::entry()?.get_password() {
                Ok(token) => Ok(Some(token)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(SessionError::Keychain(e.to_string())),
            }
        }

        fn clear(&self) -> Result<(), SessionError> {
            match Self::entry()?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(SessionError::Keychain(e.to_string())),
            }
        }

        fn backend(&self) -> Backend {
            Backend::Keychain
        }
    }
}

// ---------------------------------------------------------------------------
// File fallback
// ---------------------------------------------------------------------------

/// Stores the token in an owner-only file.
///
/// Used only when no keychain is available. The file is written through
/// [`workspace::atomic_write_private`], which creates it `0600`.
pub struct FileStore {
    path: Utf8PathBuf,
}

impl FileStore {
    pub fn new(path: impl Into<Utf8PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Utf8Path {
        &self.path
    }
}

impl SessionStore for FileStore {
    fn save(&self, refresh_token: &str) -> Result<(), SessionError> {
        workspace::atomic_write_private(&self.path, refresh_token.as_bytes())?;
        Ok(())
    }

    fn load(&self) -> Result<Option<String>, SessionError> {
        match std::fs::read_to_string(&self.path) {
            Ok(token) => {
                let trimmed = token.trim();
                Ok(if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_owned())
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(SessionError::Storage(WorkspaceError::Io {
                path: self.path.clone(),
                source,
            })),
        }
    }

    fn clear(&self) -> Result<(), SessionError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(SessionError::Storage(WorkspaceError::Io {
                path: self.path.clone(),
                source,
            })),
        }
    }

    fn backend(&self) -> Backend {
        Backend::File
    }
}

// ---------------------------------------------------------------------------
// In-memory, for tests
// ---------------------------------------------------------------------------

/// Holds the token in memory. Tests only; never persists.
#[derive(Debug, Default)]
pub struct MemoryStore {
    token: std::sync::Mutex<Option<String>>,
}

impl SessionStore for MemoryStore {
    fn save(&self, refresh_token: &str) -> Result<(), SessionError> {
        *self.token.lock().expect("session lock") = Some(refresh_token.to_owned());
        Ok(())
    }

    fn load(&self) -> Result<Option<String>, SessionError> {
        Ok(self.token.lock().expect("session lock").clone())
    }

    fn clear(&self) -> Result<(), SessionError> {
        *self.token.lock().expect("session lock") = None;
        Ok(())
    }

    fn backend(&self) -> Backend {
        Backend::Memory
    }
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

/// Returns the store to use.
///
/// Resolution order:
///
/// 1. [`SESSION_FILE_ENV`], if set. An explicit instruction wins.
/// 2. A file inside the config directory, when [`config::CONFIG_DIR_ENV`] is
///    set. Overriding the config directory means "keep this profile separate",
///    and a shared machine-wide keychain entry would silently defeat that —
///    two profiles would unknowingly share one sign-in.
/// 3. The operating-system keychain.
/// 4. A file beside the config, when no keychain is available.
///
/// The caller is expected to check [`SessionStore::backend`] and warn when the
/// result is not the keychain.
///
/// [`config::CONFIG_DIR_ENV`]: crate::config::CONFIG_DIR_ENV
pub fn open_store() -> Result<Box<dyn SessionStore>, SessionError> {
    if let Some(path) = std::env::var_os(SESSION_FILE_ENV) {
        let path = Utf8PathBuf::from_path_buf(path.into())
            .map_err(|p| SessionError::NoLocation(p.display().to_string()))?;
        return Ok(Box::new(FileStore::new(path)));
    }

    let isolated_profile = std::env::var_os(crate::config::CONFIG_DIR_ENV).is_some();

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if !isolated_profile {
        if let Some(store) = os_keychain::KeychainStore::available() {
            return Ok(Box::new(store));
        }
    }

    let directory =
        crate::config::config_dir().map_err(|e| SessionError::NoLocation(e.to_string()))?;
    let _ = isolated_profile;
    Ok(Box::new(FileStore::new(directory.join(FALLBACK_FILENAME))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_file_store() -> (TempDir, FileStore) {
        let dir = TempDir::new().expect("temp dir");
        let path =
            Utf8PathBuf::from_path_buf(std::fs::canonicalize(dir.path()).expect("canonicalize"))
                .expect("utf-8 path");
        (dir, FileStore::new(path.join("session")))
    }

    #[test]
    fn a_file_store_round_trips_a_token() {
        let (_guard, store) = temp_file_store();

        assert_eq!(store.load().unwrap(), None);
        store.save("refresh-token-value").unwrap();
        assert_eq!(
            store.load().unwrap().as_deref(),
            Some("refresh-token-value")
        );
    }

    #[test]
    fn clearing_removes_the_token() {
        let (_guard, store) = temp_file_store();
        store.save("value").unwrap();

        store.clear().unwrap();

        assert_eq!(store.load().unwrap(), None);
        assert!(!store.path().exists());
    }

    #[test]
    fn clearing_an_absent_token_is_not_an_error() {
        let (_guard, store) = temp_file_store();
        assert!(store.clear().is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn the_fallback_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let (_guard, store) = temp_file_store();
        store.save("secret").unwrap();

        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "a credential file must not be readable by other users"
        );
    }

    #[test]
    fn the_file_backend_asks_to_be_announced() {
        let (_guard, store) = temp_file_store();

        assert_eq!(store.backend(), Backend::File);
        assert!(
            store.backend().warrants_warning(),
            "falling back to a file is a real reduction in protection and must be said out loud"
        );
    }

    #[test]
    fn the_keychain_backend_needs_no_warning() {
        assert!(!Backend::Keychain.warrants_warning());
    }

    #[test]
    fn a_memory_store_round_trips_without_touching_disk() {
        let store = MemoryStore::default();

        store.save("value").unwrap();
        assert_eq!(store.load().unwrap().as_deref(), Some("value"));
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }
}
