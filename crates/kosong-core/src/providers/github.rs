//! GitHub, through `gh`.
//!
//! Permitted operations, per technical specification §12.2: `auth status`,
//! `repo view`, `repo create`, `repo sync`.
//!
//! `gh api` is named in the specification as permissible for "narrowly
//! implemented endpoints only". No variant exists for it here, because v1 has
//! no endpoint it needs. Adding one later means adding a variant with a fixed
//! path — not opening a hole through which any endpoint can be called.
//!
//! # Credentials
//!
//! Architectural rule 2 in §2: the CLI never receives or stores GitHub
//! credentials. They live in `gh`'s own credential store, and `kosong` only
//! ever asks `gh` whether it is signed in. No variant here accepts a token,
//! and none can be constructed with one.

use super::{Operation, ProviderError, validate_name};
use camino::Utf8PathBuf;
use std::time::Duration;

/// The executable. Fixed, never derived from input.
pub const PROGRAM: &str = "gh";

/// A permitted GitHub operation.
///
/// Closed by construction: there is no variant meaning "run this string", so
/// the set of things `kosong` can ask `gh` to do is exactly what is listed
/// here and is visible in one screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHubOperation {
    /// Whether `gh` is signed in. Reads nothing else.
    AuthStatus,

    /// Whether a repository exists and is reachable.
    RepoView { repo: String },

    /// Create a repository from the current folder.
    RepoCreate {
        repo: String,
        /// Private unless the user chose otherwise.
        private: bool,
        /// Push the current branch as part of creation.
        push: bool,
    },

    /// Bring the local branch in line with its remote.
    RepoSync,
}

impl GitHubOperation {
    /// Builds a `repo view` for a validated name.
    pub fn repo_view(repo: &str) -> Result<Self, ProviderError> {
        Ok(Self::RepoView {
            repo: validate_repo_reference(repo)?,
        })
    }

    /// Builds a `repo create` for a validated name.
    pub fn repo_create(repo: &str, private: bool, push: bool) -> Result<Self, ProviderError> {
        Ok(Self::RepoCreate {
            repo: validate_repo_reference(repo)?,
            private,
            push,
        })
    }
}

impl Operation for GitHubOperation {
    fn program(&self) -> &'static str {
        PROGRAM
    }

    fn args(&self) -> Vec<String> {
        // Every element is pushed separately. A name containing a space, a
        // semicolon, or a quote remains exactly one argument.
        match self {
            Self::AuthStatus => vec!["auth".into(), "status".into()],

            Self::RepoView { repo } => {
                vec!["repo".into(), "view".into(), repo.clone()]
            }

            Self::RepoCreate {
                repo,
                private,
                push,
            } => {
                let mut args = vec!["repo".into(), "create".into(), repo.clone()];
                // Visibility is always stated explicitly. Relying on `gh`'s
                // default would make the outcome depend on the user's own
                // configuration, which is not something kosong should let
                // silently decide whether a page is public.
                args.push(if *private {
                    "--private".into()
                } else {
                    "--public".into()
                });
                // Only the current directory is ever offered as the source, so
                // there is no path for a caller to point elsewhere.
                args.push("--source".into());
                args.push(".".into());
                if *push {
                    args.push("--push".into());
                }
                args
            }

            Self::RepoSync => vec!["repo".into(), "sync".into()],
        }
    }

    fn mutating(&self) -> bool {
        match self {
            Self::AuthStatus | Self::RepoView { .. } => false,
            Self::RepoCreate { .. } | Self::RepoSync => true,
        }
    }

    fn summary(&self) -> String {
        match self {
            Self::AuthStatus => "Check whether GitHub CLI is signed in.".into(),
            Self::RepoView { repo } => format!("Look up the repository `{repo}`."),
            Self::RepoCreate { repo, private, .. } => {
                let visibility = if *private { "private" } else { "public" };
                format!("Create a {visibility} GitHub repository called `{repo}`.")
            }
            Self::RepoSync => "Bring this folder in line with GitHub.".into(),
        }
    }

    fn remote_effect(&self) -> Option<String> {
        match self {
            Self::AuthStatus | Self::RepoView { .. } => None,
            Self::RepoCreate {
                repo,
                private,
                push,
            } => {
                let visibility = if *private { "private" } else { "public" };
                Some(if *push {
                    format!(
                        "creates a new {visibility} repository `{repo}` on your GitHub account, \
                         and uploads this folder to it"
                    )
                } else {
                    format!("creates a new {visibility} repository `{repo}` on your GitHub account")
                })
            }
            Self::RepoSync => Some("updates this folder from GitHub".into()),
        }
    }

    fn files(&self) -> Vec<Utf8PathBuf> {
        Vec::new()
    }

    fn timeout(&self) -> Duration {
        match self {
            // A push can be slow on a poor connection.
            Self::RepoCreate { push: true, .. } => crate::process::LONG_TIMEOUT,
            _ => crate::process::QUERY_TIMEOUT,
        }
    }
}

/// Validates a repository reference: `name` or `owner/name`.
///
/// Both halves go through the shared name validator, so a traversal attempt
/// such as `../../etc` fails on the `.` and `/` count rather than on a
/// bespoke check.
pub fn validate_repo_reference(reference: &str) -> Result<String, ProviderError> {
    let trimmed = reference.trim();

    match trimmed.split_once('/') {
        Some((owner, name)) => {
            let owner = validate_name("owner", owner)?;
            let name = validate_name("repository", name)?;
            Ok(format!("{owner}/{name}"))
        }
        None => validate_name("repository", trimmed),
    }
}

/// Reads `gh auth status` output to decide whether the user is signed in.
///
/// `gh` reports this through its exit code; the text is only used to give a
/// clearer message.
pub fn interpret_auth_status(exit_code: Option<i32>, output: &str) -> AuthState {
    if exit_code == Some(0) {
        return AuthState::SignedIn;
    }
    if output.contains("not logged") || output.contains("not logged in") {
        AuthState::SignedOut
    } else {
        AuthState::Unknown
    }
}

/// Whether `gh` is usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthState {
    SignedIn,
    SignedOut,
    /// `gh` failed for a reason that is not obviously a sign-in problem.
    Unknown,
}

impl AuthState {
    /// A plain-language next action, in the CLI's voice.
    pub fn repair(self) -> Option<&'static str> {
        match self {
            Self::SignedIn => None,
            Self::SignedOut => Some(
                "GitHub CLI is installed, but it is not signed in yet.\n\
                 Run: gh auth login\n\
                 Then come back and run: kosong doctor",
            ),
            Self::Unknown => Some(
                "GitHub CLI could not tell kosong whether it is signed in.\n\
                 Run: gh auth status",
            ),
        }
    }
}
