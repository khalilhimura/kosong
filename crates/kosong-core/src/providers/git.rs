//! Git, for the site folder's history.
//!
//! §12.2 names the permitted operations one by one — `init`, `status`,
//! `rev-parse`, `config --get`, `ls-files`, `add`, `commit --only`, `push` —
//! and requires `git` be "called by `site init/publish` with known file paths
//! only". So there is no variant that stages everything: the paths come from
//! the template's own manifest, and a file the user dropped into the folder is
//! never committed by accident.
//!
//! It used to describe that set rather than list it, which is how the list grew
//! to eight without the document noticing. Adding a variant here means adding a
//! row there.
//!
//! Several variants only ask git a question. They are held to the same rule —
//! a fixed subcommand, and where paths are involved, the same manifest list the
//! mutating variants get.

use super::{Operation, ProviderError, validate_name};
use camino::{Utf8Path, Utf8PathBuf};

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

    /// Which of these paths git already has in its index.
    ///
    /// Read-only, and the answer existence on disk cannot give: a file the
    /// user deleted is gone from the folder but still tracked, and staging
    /// that deletion is the only way the history keeps up with the folder.
    ListTracked { paths: Vec<Utf8PathBuf> },

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

    pub fn list_tracked(paths: Vec<Utf8PathBuf>) -> Self {
        Self::ListTracked { paths }
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

            Self::ListTracked { paths } => {
                // No `--error-unmatch`: a path git has never heard of is the
                // expected answer here, not a failure. Without that flag
                // `ls-files` exits 0 and simply says nothing about it.
                let mut args = vec!["ls-files".into(), "--".into()];
                args.extend(paths.iter().map(ToString::to_string));
                args
            }

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
            Self::Status | Self::CurrentBranch | Self::IdentityCheck | Self::ListTracked { .. } => {
                false
            }
            Self::Init | Self::Add { .. } | Self::Commit { .. } | Self::Push { .. } => true,
        }
    }

    fn summary(&self) -> String {
        match self {
            Self::Init => "Start tracking this folder's history.".into(),
            Self::Status => "Check for unsaved changes.".into(),
            Self::CurrentBranch => "Check which branch this is.".into(),
            Self::IdentityCheck => "Check that git knows who you are.".into(),
            Self::ListTracked { paths } => {
                format!("Check which of {} file(s) git already tracks.", paths.len())
            }
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

/// Whether [`GitOperation::ListTracked`] listed `path`, or anything inside it.
///
/// An owned path may name a directory — `src/content` — which `ls-files`
/// answers with the files under it rather than with the directory, so a prefix
/// counts. That is the same rule [`unrelated_changes`] applies, for the same
/// reason.
///
/// `ls-files` quotes any path holding a byte outside ASCII, per
/// `core.quotePath`. Every owned path is ASCII, so quoting can only ever wrap a
/// name *underneath* one — and the leading `src/content/` a prefix match needs
/// survives inside the quotes intact.
pub fn tracks(listed: &str, path: &Utf8Path) -> bool {
    let path = path.as_str();
    let prefix = format!("{path}/");

    listed.lines().any(|line| {
        let line = line.trim_matches('"');
        line == path || line.starts_with(&prefix)
    })
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

/// Whether a failed push failed because git could not sign in.
///
/// Signing in to GitHub and signing in to *git* are two different things, and
/// kosong can be in the state where only the first is done: `gh auth status`
/// passes, so a repository is created and recorded, and then every push fails.
/// A GitHub Actions runner is in that state by default, and so is any user who
/// answered no when `gh auth login` offered to configure git, or who
/// authenticated with `GH_TOKEN` alone.
///
/// Matched on the phrases git uses rather than on an exit code, because the
/// code is the same 128 it returns for a bad remote. The trailing reason varies
/// by platform — `Device not configured`, `No such device or address`,
/// `terminal prompts disabled` — so none of those can be the thing matched.
pub fn needs_credentials(output: &str) -> bool {
    // `Authentication failed` covers the other half: a helper that answers with
    // a token GitHub no longer accepts.
    const SIGNS: [&str; 3] = [
        "could not read Username",
        "could not read Password",
        "Authentication failed",
    ];

    SIGNS.iter().any(|sign| output.contains(sign))
}

/// What to tell a user whose git cannot sign in to GitHub.
pub const PUSH_REPAIR: &str = "\
Your page is published — this only affects the copy of your history on GitHub.

Signing in to GitHub and signing in to git are two different things, and only
the first one is done. To connect the second, run this once:
  gh auth setup-git
If that says you are not logged in, run `gh auth login` first, then try it again.
Then run `kosong site publish` again to send your history up.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_push_with_no_credential_helper_is_recognised() {
        // Both captured by running `git push` against an https remote with no
        // helper and stdin closed — exactly how kosong spawns it. Which of the
        // two trailing reasons appears depends on the platform, so neither can
        // be the thing matched on.
        assert!(needs_credentials(
            "fatal: could not read Username for 'https://github.com': Device not configured"
        ));
        assert!(needs_credentials(
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled"
        ));
        // A helper that answers with a stale or wrong token fails later, and
        // lands the user in the same place.
        assert!(needs_credentials(
            "remote: Support for password authentication was removed.\n\
             fatal: Authentication failed for 'https://github.com/me/site.git/'"
        ));
    }

    #[test]
    fn the_repair_covers_a_gh_session_that_has_since_expired() {
        // `gh auth setup-git` refuses when no host is authenticated —
        // "You are not logged into any GitHub hosts" — and that is reachable
        // here. The repository is recorded at `site init`, when gh was signed
        // in; the push that fails can be days later, when it is not. Naming
        // only `setup-git` would strand exactly the user this message exists
        // for, one step short.
        assert!(PUSH_REPAIR.contains("gh auth setup-git"));
        assert!(PUSH_REPAIR.contains("gh auth login"));

        // Order matters as much as presence: `setup-git` is the usual fix, and
        // `login` is the fallback when it refuses. Reversed, every reader is
        // sent to re-authenticate a session that is probably fine.
        let setup = PUSH_REPAIR.find("gh auth setup-git").expect("setup-git");
        let login = PUSH_REPAIR.find("gh auth login").expect("login");
        assert!(setup < login, "setup-git is the first thing to try");
    }

    #[test]
    fn an_ordinary_push_failure_is_not_blamed_on_credentials() {
        // Sending someone to set up credentials when the truth is a rejected
        // non-fast-forward costs them more time than saying nothing would.
        assert!(!needs_credentials(
            "! [rejected] main -> main (fetch first)\nerror: failed to push some refs"
        ));
        assert!(!needs_credentials(
            "fatal: 'origin' does not appear to be a git repository"
        ));
        assert!(!needs_credentials("Everything up-to-date"));
        assert!(!needs_credentials(""));
    }
}
