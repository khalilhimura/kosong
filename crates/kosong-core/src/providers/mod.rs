//! Narrow, typed access to external tools.
//!
//! Technical specification §12.2 defines an allowlist, and §6.2 puts
//! "arbitrary pass-through support for every `gh` or `wrangler` command"
//! explicitly out of scope.
//!
//! The allowlist is therefore expressed as a **closed enum per provider**, not
//! a list of permitted strings. An operation that is not a variant cannot be
//! constructed, so there is no filtering step to get wrong and no way for a
//! caller to smuggle a subcommand through. Argument vectors are built by the
//! enum itself; a caller supplies typed values, never argv.

pub mod cloudflare;
pub mod git;
pub mod github;
pub mod npm;

use crate::process::{SafeCommand, format_command};
use camino::Utf8PathBuf;

/// Longest accepted name for a repository or project.
const MAX_NAME_LENGTH: usize = 58;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("`{name}` is not a usable {kind} name")]
    InvalidName { kind: &'static str, name: String },

    #[error("`{name}` is too long for a {kind} name (limit {MAX_NAME_LENGTH})")]
    NameTooLong { kind: &'static str, name: String },

    #[error("`{name}` is not a usable Cloudflare project name")]
    InvalidProjectName { name: String },
}

impl ProviderError {
    pub fn repair(&self) -> String {
        match self {
            // Phrased without an article, because the kind may begin with
            // either a consonant or a vowel ("a project", "an owner").
            Self::InvalidName { kind, .. } => format!(
                "Names for a {kind} may use letters, numbers, and hyphens, and must start and \
                 end with a letter or number. Try something like `my-first-site`."
            ),
            Self::NameTooLong { kind, .. } => {
                format!("Shorten the {kind} name to {MAX_NAME_LENGTH} characters or fewer.")
            }
            Self::InvalidProjectName { .. } => "Cloudflare project names use lowercase letters, \
                 numbers, and hyphens only — no capitals and no underscores — and must start and \
                 end with a letter or number. Try something like `my-first-site`.\n\
                 Change it in `.kosong/site.toml`, or run `kosong site init` again with a \
                 different name."
                .into(),
        }
    }
}

/// Validates a repository or project name.
///
/// Deliberately stricter than either provider requires. Names reaching here
/// come from user input and end up as command arguments and URL components, so
/// the accepted set is narrowed to what is unambiguous everywhere rather than
/// to the union of what each provider tolerates.
///
/// Note this is defence in depth, not the primary control: nothing here ever
/// reaches a shell, so a rejected character would have been inert anyway.
pub fn validate_name(kind: &'static str, name: &str) -> Result<String, ProviderError> {
    let trimmed = name.trim();

    if trimmed.is_empty() || trimmed.len() > MAX_NAME_LENGTH {
        return if trimmed.len() > MAX_NAME_LENGTH {
            Err(ProviderError::NameTooLong {
                kind,
                name: trimmed.to_owned(),
            })
        } else {
            Err(ProviderError::InvalidName {
                kind,
                name: trimmed.to_owned(),
            })
        };
    }

    let shape_is_fine = trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && trimmed.starts_with(|c: char| c.is_ascii_alphanumeric())
        && trimmed.ends_with(|c: char| c.is_ascii_alphanumeric());

    if !shape_is_fine {
        return Err(ProviderError::InvalidName {
            kind,
            name: trimmed.to_owned(),
        });
    }
    Ok(trimmed.to_owned())
}

/// What an operation is about to do, before it does it.
///
/// §12.4 requires a mutating operation to print its executable, quoted argv,
/// working directory, the files involved, and its remote effect — then stop
/// on `--dry-run`, or ask for confirmation.
#[derive(Debug, Clone)]
pub struct OperationPlan {
    /// One line, in plain language.
    pub summary: String,
    pub executable: String,
    /// Where `executable` was found, when that is not left to `PATH`.
    ///
    /// `None` — the usual case — means the disclosure says nothing extra, so a
    /// machine with no project-local install reads exactly what it read before.
    pub resolved: Option<Utf8PathBuf>,
    pub args: Vec<String>,
    pub cwd: Utf8PathBuf,
    /// Local files this will read, write, or commit.
    pub files: Vec<Utf8PathBuf>,
    /// What changes outside this computer, if anything.
    pub remote_effect: Option<String>,
    /// Whether this changes anything. Read-only plans need no confirmation.
    pub mutating: bool,
}

impl OperationPlan {
    /// Records where the allowlisted program was actually found.
    ///
    /// [`Self::display_command`] is left alone: the command line still reads as
    /// the tool a person recognises, and the path is disclosed on its own line.
    pub fn found_at(mut self, path: impl Into<Utf8PathBuf>) -> Self {
        self.resolved = Some(path.into());
        self
    }

    /// The invocation as a person would read it, redacted.
    pub fn display_command(&self) -> String {
        format_command(&self.executable, &self.args)
    }

    /// The full disclosure §12.4 requires, as lines to print.
    pub fn disclosure(&self) -> Vec<String> {
        let mut lines = vec![
            self.summary.clone(),
            String::new(),
            format!("  run     {}", self.display_command()),
            format!("  in      {}", self.cwd),
        ];

        // Only when it is not the plain `PATH` hit. §12.4 asks the disclosure to
        // describe what will really run, and where two copies of a tool could be
        // on the machine, the name alone does not say which one this is.
        if let Some(resolved) = &self.resolved {
            lines.push(format!("  using   {resolved}"));
        }

        if !self.files.is_empty() {
            lines.push(format!("  files   {}", self.files.len()));
            for file in &self.files {
                lines.push(format!("            {file}"));
            }
        }

        match &self.remote_effect {
            Some(effect) => lines.push(format!("  effect  {effect}")),
            None => lines.push("  effect  nothing outside this computer".to_owned()),
        }
        lines
    }
}

/// An operation that can describe itself and build its own invocation.
pub trait Operation {
    /// The executable to run. Always a fixed string from the allowlist.
    fn program(&self) -> &'static str;

    /// The argument vector. Built here, never supplied by a caller.
    fn args(&self) -> Vec<String>;

    /// Whether this changes local or remote state.
    fn mutating(&self) -> bool;

    /// One line of plain language describing the intent.
    fn summary(&self) -> String;

    /// What changes outside this computer, if anything.
    fn remote_effect(&self) -> Option<String> {
        None
    }

    /// Local files involved.
    fn files(&self) -> Vec<Utf8PathBuf> {
        Vec::new()
    }

    /// How long to allow before giving up.
    fn timeout(&self) -> std::time::Duration {
        crate::process::QUERY_TIMEOUT
    }

    /// Builds the plan to show the user.
    fn plan(&self, cwd: &camino::Utf8Path) -> OperationPlan {
        OperationPlan {
            summary: self.summary(),
            executable: self.program().to_owned(),
            resolved: None,
            args: self.args(),
            cwd: cwd.to_owned(),
            files: self.files(),
            remote_effect: self.remote_effect(),
            mutating: self.mutating(),
        }
    }

    /// Builds the invocation. Running it is the caller's decision.
    fn command(&self, cwd: &camino::Utf8Path) -> SafeCommand {
        SafeCommand::new(self.program(), cwd)
            .args(self.args())
            .timeout(self.timeout())
    }
}
