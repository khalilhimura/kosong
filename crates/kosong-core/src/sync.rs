//! Deciding what a sync should do, and preserving both sides when it cannot.
//!
//! FR-4 sets the rules: conflicts are visible and non-destructive, v1 must not
//! silently merge concurrent changes, and a bare `kosong sync` must explain
//! its detected action or the conflict.
//!
//! [`plan`] is deliberately pure. Given what is local, what is remote, and
//! what was true at the last sync, it returns an action. No network, no
//! filesystem, no clock — so every branch, including the ones that are hard to
//! reach against a live server, is directly testable.
//!
//! # How change is detected
//!
//! - **Local** change: the document's content hash differs from the hash
//!   recorded at the last successful sync.
//! - **Remote** change: the server's etag differs from the one recorded at the
//!   last successful sync.
//!
//! When both changed, that is a conflict. There is no correct automatic merge
//! of two versions of someone's prose, so both are written to disk and the
//! user decides.

use crate::workspace::{Workspace, WorkspaceError};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Directory under `.kosong` holding preserved conflict snapshots.
const CONFLICTS_DIR: &str = "conflicts";

/// Filename for sync state within `.kosong`.
const STATE_FILENAME: &str = "sync.toml";

/// What was true at the end of the last successful sync.
///
/// Non-secret: a content hash and an opaque version token. Never a document
/// body, never a credential.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncState {
    /// The server's version token as of the last sync.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// Hash of the local document as of the last sync.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// When that sync happened, RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synced_at: Option<String>,
}

impl SyncState {
    pub fn path(workspace: &Workspace) -> Utf8PathBuf {
        workspace.state_dir().join(STATE_FILENAME)
    }

    /// Loads state, treating an absent or unreadable file as "never synced".
    ///
    /// A corrupt state file must not block a sync: the worst outcome of
    /// forgetting it is a conflict the user resolves by hand, which is safe.
    /// Refusing to sync would not be.
    pub fn load(workspace: &Workspace) -> Self {
        std::fs::read_to_string(Self::path(workspace))
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, workspace: &Workspace) -> Result<(), WorkspaceError> {
        let text = toml::to_string_pretty(self).map_err(|e| WorkspaceError::Io {
            path: Self::path(workspace),
            source: std::io::Error::other(e.to_string()),
        })?;
        crate::workspace::atomic_write(&Self::path(workspace), text.as_bytes())
    }
}

/// SHA-256 of document bytes, hex encoded.
pub fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The local side of a sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSide {
    pub hash: String,
}

/// The remote side of a sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSide {
    pub etag: String,
    /// Hash of the remote bytes, when they have been fetched.
    pub hash: Option<String>,
}

/// What a sync should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncAction {
    /// Neither side has a document.
    NothingToSync,
    /// Both sides already agree.
    UpToDate,
    /// Send the local document. `if_match` is `"*"` or the known remote etag.
    Push { if_match: String },
    /// Take the remote document.
    Pull,
    /// Both changed since the last sync. Nothing is overwritten.
    Conflict,
}

impl SyncAction {
    /// A plain-language description of what is about to happen.
    pub fn describe(&self) -> &'static str {
        match self {
            Self::NothingToSync => "There is no page here and none on the server.",
            Self::UpToDate => "Your page and the server already match.",
            Self::Push { .. } => "Your page is newer, so it will be sent to the server.",
            Self::Pull => "The server has a newer page, so it will be copied here.",
            Self::Conflict => "Your page and the server's have both changed since the last sync.",
        }
    }
}

/// Works out what a sync should do.
///
/// `last` is the state recorded at the end of the previous successful sync.
/// An empty `last` means this workspace has never synced.
pub fn plan(
    local: Option<&LocalSide>,
    remote: Option<&RemoteSide>,
    last: &SyncState,
) -> SyncAction {
    match (local, remote) {
        (None, None) => SyncAction::NothingToSync,

        // First upload.
        (Some(_), None) => SyncAction::Push {
            if_match: "*".to_owned(),
        },

        // Nothing local to lose, so taking the remote is safe.
        (None, Some(_)) => SyncAction::Pull,

        (Some(local), Some(remote)) => {
            // Identical content is agreement, whatever the recorded state
            // says. This is what stops a lost state file from manufacturing a
            // conflict between two copies that are byte-for-byte the same.
            if remote.hash.as_deref() == Some(local.hash.as_str()) {
                return SyncAction::UpToDate;
            }

            let local_changed = last.content_hash.as_deref() != Some(local.hash.as_str());
            let remote_changed = last.etag.as_deref() != Some(remote.etag.as_str());

            match (local_changed, remote_changed) {
                (false, false) => SyncAction::UpToDate,
                (true, false) => SyncAction::Push {
                    if_match: remote.etag.clone(),
                },
                (false, true) => SyncAction::Pull,
                // Includes the never-synced case, where both read as changed.
                // Treating an unknown history as a conflict is the safe
                // default: it costs the user a decision, not their work.
                (true, true) => SyncAction::Conflict,
            }
        }
    }
}

/// Where a conflict's two versions were written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictSnapshot {
    pub directory: Utf8PathBuf,
    pub local_path: Utf8PathBuf,
    pub remote_path: Utf8PathBuf,
}

/// Writes both versions to a timestamped directory inside the workspace.
///
/// Neither the working document nor the server is touched. §9.3 requires the
/// snapshots be separately named and kept in the workspace, so the user can
/// open both in their own editor and merge by hand.
pub fn write_conflict(
    workspace: &Workspace,
    local_bytes: &[u8],
    remote_bytes: &[u8],
    timestamp: &str,
) -> Result<ConflictSnapshot, WorkspaceError> {
    // Colons are legal on Unix but awkward everywhere else, so the timestamp
    // is flattened into something safe to type and to tab-complete.
    let safe = timestamp.replace([':', '.', '+'], "-");
    let directory = workspace.state_dir().join(CONFLICTS_DIR).join(safe);

    let local_path = directory.join("your-version.md");
    let remote_path = directory.join("server-version.md");

    crate::workspace::atomic_write(&local_path, local_bytes)?;
    crate::workspace::atomic_write(&remote_path, remote_bytes)?;

    Ok(ConflictSnapshot {
        directory,
        local_path,
        remote_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(hash: &str) -> LocalSide {
        LocalSide {
            hash: hash.to_owned(),
        }
    }

    fn remote(etag: &str, hash: Option<&str>) -> RemoteSide {
        RemoteSide {
            etag: etag.to_owned(),
            hash: hash.map(str::to_owned),
        }
    }

    fn synced(etag: &str, hash: &str) -> SyncState {
        SyncState {
            etag: Some(etag.to_owned()),
            content_hash: Some(hash.to_owned()),
            synced_at: None,
        }
    }

    #[test]
    fn nothing_anywhere_is_nothing_to_do() {
        assert_eq!(
            plan(None, None, &SyncState::default()),
            SyncAction::NothingToSync
        );
    }

    #[test]
    fn a_first_upload_pushes_with_a_wildcard() {
        assert_eq!(
            plan(Some(&local("abc")), None, &SyncState::default()),
            SyncAction::Push {
                if_match: "*".to_owned()
            }
        );
    }

    #[test]
    fn nothing_local_pulls() {
        assert_eq!(
            plan(None, Some(&remote("etag-1", None)), &SyncState::default()),
            SyncAction::Pull
        );
    }

    #[test]
    fn matching_content_is_up_to_date_even_with_no_recorded_state() {
        // A lost or corrupt state file must not manufacture a conflict between
        // two copies that are byte-for-byte identical.
        assert_eq!(
            plan(
                Some(&local("same-hash")),
                Some(&remote("etag-1", Some("same-hash"))),
                &SyncState::default()
            ),
            SyncAction::UpToDate
        );
    }

    #[test]
    fn no_change_on_either_side_is_up_to_date() {
        assert_eq!(
            plan(
                Some(&local("abc")),
                Some(&remote("etag-1", None)),
                &synced("etag-1", "abc")
            ),
            SyncAction::UpToDate
        );
    }

    #[test]
    fn a_local_edit_pushes_against_the_known_etag() {
        assert_eq!(
            plan(
                Some(&local("changed")),
                Some(&remote("etag-1", None)),
                &synced("etag-1", "abc")
            ),
            SyncAction::Push {
                if_match: "etag-1".to_owned()
            }
        );
    }

    #[test]
    fn a_remote_change_pulls() {
        assert_eq!(
            plan(
                Some(&local("abc")),
                Some(&remote("etag-2", None)),
                &synced("etag-1", "abc")
            ),
            SyncAction::Pull
        );
    }

    #[test]
    fn changes_on_both_sides_are_a_conflict() {
        assert_eq!(
            plan(
                Some(&local("changed")),
                Some(&remote("etag-2", Some("also-changed"))),
                &synced("etag-1", "abc")
            ),
            SyncAction::Conflict
        );
    }

    #[test]
    fn a_never_synced_workspace_with_both_sides_is_a_conflict() {
        // Safe default: an unknown history costs the user a decision, not
        // their work.
        assert_eq!(
            plan(
                Some(&local("local-hash")),
                Some(&remote("etag-1", Some("remote-hash"))),
                &SyncState::default()
            ),
            SyncAction::Conflict
        );
    }

    #[test]
    fn every_action_explains_itself() {
        for action in [
            SyncAction::NothingToSync,
            SyncAction::UpToDate,
            SyncAction::Push {
                if_match: "*".into(),
            },
            SyncAction::Pull,
            SyncAction::Conflict,
        ] {
            assert!(!action.describe().is_empty());
            assert!(action.describe().ends_with('.'));
        }
    }

    #[test]
    fn content_hash_is_stable_and_distinguishing() {
        assert_eq!(content_hash(b"same"), content_hash(b"same"));
        assert_ne!(content_hash(b"one"), content_hash(b"two"));
        assert_eq!(content_hash(b"").len(), 64);
    }

    #[test]
    fn sync_state_holds_no_secret() {
        let state = SyncState {
            etag: Some("etag-1".into()),
            content_hash: Some("abc".into()),
            synced_at: Some("2026-07-26T10:00:00+08:00".into()),
        };
        let text = toml::to_string_pretty(&state).unwrap();

        for forbidden in ["token", "secret", "password", "@"] {
            assert!(
                !text.to_lowercase().contains(forbidden),
                "sync state must never contain `{forbidden}`"
            );
        }
    }
}
