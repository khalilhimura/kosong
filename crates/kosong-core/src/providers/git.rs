//! Git, for the site folder's history.
//!
//! §12.2 permits `git` "initialized and called by `site init/publish` with
//! known file paths only". So there is no variant that stages everything: the
//! paths come from the template's own manifest, and a file the user dropped
//! into the folder is never committed by accident.

use super::{Operation, ProviderError, validate_name};
use camino::Utf8PathBuf;

/// The executable. Fixed, never derived from input.
pub const PROGRAM: &str = "git";

/// The branch a new site starts on.
pub const DEFAULT_BRANCH: &str = "main";

/// A permitted git operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitOperation {
    /// Start a repository on [`DEFAULT_BRANCH`].
    Init,

    /// Machine-readable working tree state, for the dirty check.
    Status,

    /// Which branch is checked out.
    CurrentBranch,

    /// Whether committer identity is configured. Without it, `commit` fails
    /// with a message that baffles a beginner.
    IdentityCheck,

    /// Stage an explicit list of paths. Never `.`, never `-A`.
    Add { paths: Vec<Utf8PathBuf> },

    /// Record a commit of exactly these paths.
    ///
    /// The paths are repeated here rather than relying on what is staged,
    /// because a user may have staged something of their own before running
    /// kosong, and a bare `git commit` would sweep it in.
    Commit {
        message: String,
        paths: Vec<Utf8PathBuf>,
    },

    /// Send a branch to a remote.
    Push { remote: String, branch: String },
}

impl GitOperation {
    pub fn add(paths: Vec<Utf8PathBuf>) -> Self {
        Self::Add { paths }
    }

    pub fn push(remote: &str, branch: &str) -> Result<Self, ProviderError> {
        Ok(Self::Push {
            remote: validate_name("remote", remote)?,
            branch: validate_name("branch", branch)?,
        })
    }
}

impl Operation for GitOperation {
    fn program(&self) -> &'static str {
        PROGRAM
    }

    fn args(&self) -> Vec<String> {
        match self {
            Self::Init => vec![
                "init".into(),
                // Stated explicitly rather than inheriting the user's
                // `init.defaultBranch`, so the branch kosong later pushes is
                // the branch it created.
                "--initial-branch".into(),
                DEFAULT_BRANCH.into(),
            ],

            Self::Status => vec!["status".into(), "--porcelain".into()],

            Self::CurrentBranch => {
                vec!["rev-parse".into(), "--abbrev-ref".into(), "HEAD".into()]
            }

            Self::IdentityCheck => vec!["config".into(), "--get".into(), "user.email".into()],

            Self::Add { paths } => {
                let mut args = vec!["add".into(), "--".into()];
                // `--` ends option parsing, so a path beginning with a hyphen
                // is treated as a path rather than a flag.
                args.extend(paths.iter().map(ToString::to_string));
                args
            }

            Self::Commit { message, paths } => {
                let mut args = vec!["commit".into(), "--message".into(), message.clone()];
                // `--only <paths>` commits exactly these, ignoring whatever
                // else happens to be staged. It requires the paths to be
                // given: `--only` alone is an error, not a shorthand.
                args.push("--only".into());
                args.push("--".into());
                args.extend(paths.iter().map(ToString::to_string));
                args
            }

            Self::Push { remote, branch } => vec![
                "push".into(),
                "--set-upstream".into(),
                remote.clone(),
                branch.clone(),
            ],
        }
    }

    fn mutating(&self) -> bool {
        match self {
            Self::Status | Self::CurrentBranch | Self::IdentityCheck => false,
            Self::Init | Self::Add { .. } | Self::Commit { .. } | Self::Push { .. } => true,
        }
    }

    fn summary(&self) -> String {
        match self {
            Self::Init => "Start tracking this folder's history.".into(),
            Self::Status => "Check for unsaved changes.".into(),
            Self::CurrentBranch => "Check which branch this is.".into(),
            Self::IdentityCheck => "Check that git knows who you are.".into(),
            Self::Add { paths } => format!("Stage {} file(s).", paths.len()),
            Self::Commit { paths, .. } => {
                format!("Record {} file(s) in this folder's history.", paths.len())
            }
            Self::Push { remote, branch } => format!("Send `{branch}` to `{remote}`."),
        }
    }

    fn remote_effect(&self) -> Option<String> {
        match self {
            Self::Push { remote, .. } => {
                Some(format!("uploads your site folder to `{remote}` on GitHub"))
            }
            // Init, add, and commit all change only this computer.
            _ => None,
        }
    }

    fn files(&self) -> Vec<Utf8PathBuf> {
        match self {
            Self::Add { paths } | Self::Commit { paths, .. } => paths.clone(),
            _ => Vec::new(),
        }
    }

    fn timeout(&self) -> std::time::Duration {
        match self {
            Self::Push { .. } => crate::process::LONG_TIMEOUT,
            _ => crate::process::QUERY_TIMEOUT,
        }
    }
}

/// A working tree change reported by `git status --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The two-character status code, e.g. `??` or ` M`.
    pub code: String,
    pub path: String,
}

/// Parses `git status --porcelain` output.
///
/// The format is `XY <path>`, with the two status characters in fixed columns.
/// Renames appear as `old -> new`; only the new path is kept, which is what
/// matters for deciding whether a file is ours.
pub fn parse_status(output: &str) -> Vec<Change> {
    output
        .lines()
        .filter(|line| line.len() > 3)
        .map(|line| {
            let (code, rest) = line.split_at(2);
            let path = rest.trim();
            let path = path.rsplit(" -> ").next().unwrap_or(path);
            Change {
                code: code.to_owned(),
                path: path.trim_matches('"').to_owned(),
            }
        })
        .collect()
}

/// Changes that do not belong to the generated site.
///
/// §13.3 step 3: publish must refuse by default when the repository has
/// unrelated uncommitted changes. "Unrelated" means anything outside the
/// template's own manifest — committing a user's stray file because it
/// happened to be in the folder would be a surprise, and possibly a leak.
pub fn unrelated_changes(output: &str, owned: &[Utf8PathBuf]) -> Vec<Change> {
    let owned: Vec<String> = owned.iter().map(ToString::to_string).collect();

    parse_status(output)
        .into_iter()
        .filter(|change| {
            !owned
                .iter()
                .any(|path| change.path == *path || change.path.starts_with(&format!("{path}/")))
        })
        .collect()
}

/// Whether `git config user.email` produced a usable identity.
pub fn has_identity(exit_code: Option<i32>, output: &str) -> bool {
    exit_code == Some(0) && !output.trim().is_empty()
}

/// What to tell a user whose git identity is not set.
pub const IDENTITY_REPAIR: &str = "\
Git needs to know who you are before it can record history.
Run these two, with your own name and email:
  git config --global user.name \"Your Name\"
  git config --global user.email \"you@example.com\"
Then try again.";
