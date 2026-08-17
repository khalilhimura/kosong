//! Running external tools safely.
//!
//! # The rule
//!
//! Technical specification §12.1: every child process is an executable plus
//! **separate argument values**. Never a command string.
//!
//! Forbidden outright: `sh -c`, `bash -c`, `zsh -c`, `cmd.exe /c`, PowerShell
//! expression invocation, parsing a command string out of user input, pipes,
//! redirection, shell expansion, and hidden background execution.
//!
//! This module is the only place in `kosong` that spawns a process, and it
//! offers no way to express any of those. There is no `sh` anywhere in the
//! codebase, so a shell metacharacter arriving in a repository name is an
//! inert character in an argument vector — not syntax. The tests in
//! `tests/process_safety.rs` demonstrate this against real fake binaries
//! rather than asserting it in prose.
//!
//! # What this does provide
//!
//! - A bounded timeout, so a hung provider cannot hang the CLI.
//! - A typed [`ProcessResult`] carrying everything needed to explain what ran.
//! - Redaction of token-shaped output, applied before anything is displayed or
//!   logged. (See [`crate::redact`].)

use crate::redact;
use camino::{Utf8Path, Utf8PathBuf};
use std::time::{Duration, Instant};

/// Default limit for a quick query, such as `gh auth status`.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

/// Default limit for something slow, such as a deploy or an install.
pub const LONG_TIMEOUT: Duration = Duration::from_secs(600);

/// What a finished child process did.
///
/// Carries enough to reconstruct the invocation for `--dry-run` output, an
/// error message, or a log line.
#[derive(Debug, Clone)]
pub struct ProcessResult {
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: Utf8PathBuf,
    /// `None` when the process was killed by a signal.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration: Duration,
}

impl ProcessResult {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// The invocation as a person would read it, redacted.
    pub fn display_command(&self) -> String {
        format_command(&self.executable, &self.args)
    }

    /// Combined output, redacted and trimmed. For showing a failure.
    pub fn combined_output(&self) -> String {
        let mut combined = String::new();
        if !self.stderr.trim().is_empty() {
            combined.push_str(self.stderr.trim());
        }
        if !self.stdout.trim().is_empty() {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(self.stdout.trim());
        }
        combined
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("`{program}` is not installed")]
    NotFound { program: String },

    #[error("`{program}` did not finish within {seconds} seconds")]
    Timeout { program: String, seconds: u64 },

    #[error("could not run `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },

    #[error("`{program}` failed")]
    Failed {
        program: String,
        result: Box<ProcessResult>,
    },
}

impl ProcessError {
    /// A plain-language next action, in the CLI's voice.
    pub fn repair(&self) -> String {
        match self {
            Self::NotFound { program } => {
                format!("Install `{program}`, then run: kosong doctor")
            }
            Self::Timeout { program, .. } => {
                format!(
                    "`{program}` may be waiting for input, or the network may be slow. Try again."
                )
            }
            Self::Spawn { program, .. } => {
                format!("Check that `{program}` is installed and you can run it.")
            }
            Self::Failed { result, .. } => {
                let output = result.combined_output();
                if output.is_empty() {
                    "The tool reported no reason. Try running it yourself to see.".into()
                } else {
                    output
                }
            }
        }
    }
}

/// An invocation waiting to be run.
///
/// Built up from an executable name and discrete arguments. There is
/// deliberately no constructor that takes a whole command line.
#[derive(Debug, Clone)]
pub struct SafeCommand {
    program: String,
    /// Where `program` was found, when that is not left to `PATH`.
    ///
    /// Only [`Self::run`] reads this. Everything a person sees — [`Self::display`],
    /// the error variants, [`ProcessResult::executable`] — keeps the allowlisted
    /// name, because "`wrangler` failed" is what a reader can act on and an
    /// absolute path into `node_modules` is not. §12.4's disclosure names the
    /// path separately, which is where that detail belongs.
    resolved: Option<Utf8PathBuf>,
    args: Vec<String>,
    cwd: Utf8PathBuf,
    timeout: Duration,
}

impl SafeCommand {
    /// Starts building an invocation.
    ///
    /// `program` comes from a provider allowlist, never from user input.
    pub fn new(program: impl Into<String>, cwd: impl Into<Utf8PathBuf>) -> Self {
        Self {
            program: program.into(),
            resolved: None,
            args: Vec::new(),
            cwd: cwd.into(),
            timeout: QUERY_TIMEOUT,
        }
    }

    /// Spawns the program from `path` rather than letting `PATH` find it.
    ///
    /// This does **not** widen what may be run. The program is still the
    /// allowlisted name from a provider; `path` is where that name was resolved
    /// to, which is how a tool installed as a project dev dependency — the way
    /// Cloudflare documents `wrangler` — becomes reachable at all. Callers pass
    /// this only when the resolved path is not what `PATH` would have given
    /// anyway, so a machine with no project-local install builds exactly the
    /// invocation it built before.
    pub fn found_at(mut self, path: impl Into<Utf8PathBuf>) -> Self {
        self.resolved = Some(path.into());
        self
    }

    /// What [`Self::run`] will hand to the operating system.
    ///
    /// The resolved path when there is one, otherwise the bare name for `PATH`
    /// to resolve. Distinct from [`Self::program`], which is always the name.
    pub fn executable(&self) -> &str {
        self.resolved
            .as_ref()
            .map_or(self.program.as_str(), |path| path.as_str())
    }

    /// Adds one argument.
    ///
    /// One call, one argument. A value containing spaces stays a single
    /// argument, and a value containing `;` or `&&` stays inert text.
    pub fn arg(mut self, value: impl Into<String>) -> Self {
        self.args.push(value.into());
        self
    }

    pub fn args<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(values.into_iter().map(Into::into));
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn program(&self) -> &str {
        &self.program
    }

    pub fn arg_list(&self) -> &[String] {
        &self.args
    }

    pub fn cwd(&self) -> &Utf8Path {
        &self.cwd
    }

    /// The invocation as a person would read it, redacted and quoted.
    pub fn display(&self) -> String {
        format_command(&self.program, &self.args)
    }

    /// Runs the process to completion.
    ///
    /// Uses `tokio::process` so stdout and stderr are drained while waiting.
    /// A naive `wait()` with piped output deadlocks the moment a child fills a
    /// pipe buffer, which for a verbose deploy is routine rather than exotic.
    pub async fn run(&self) -> Result<ProcessResult, ProcessError> {
        let started = Instant::now();

        let mut command = tokio::process::Command::new(self.executable());
        command
            .args(&self.args)
            .current_dir(&self.cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Stops a child from inheriting anything that looks like a prompt
            // hint and deciding to be interactive.
            .env("CI", "1")
            .kill_on_drop(true);

        let child = command.spawn().map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                ProcessError::NotFound {
                    program: self.program.clone(),
                }
            } else {
                ProcessError::Spawn {
                    program: self.program.clone(),
                    source,
                }
            }
        })?;

        let output = match tokio::time::timeout(self.timeout, child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(source)) => {
                return Err(ProcessError::Spawn {
                    program: self.program.clone(),
                    source,
                });
            }
            Err(_elapsed) => {
                // `kill_on_drop` reaps the child when `child` is dropped here,
                // so a timed-out process is not left running.
                return Err(ProcessError::Timeout {
                    program: self.program.clone(),
                    seconds: self.timeout.as_secs(),
                });
            }
        };

        Ok(ProcessResult {
            executable: self.program.clone(),
            args: self.args.clone(),
            cwd: self.cwd.clone(),
            exit_code: output.status.code(),
            // Redacted on the way in, so no caller can accidentally handle raw
            // token-shaped output.
            stdout: redact::redact(&String::from_utf8_lossy(&output.stdout)),
            stderr: redact::redact(&String::from_utf8_lossy(&output.stderr)),
            duration: started.elapsed(),
        })
    }

    /// Runs, and treats a non-zero exit as an error.
    pub async fn run_checked(&self) -> Result<ProcessResult, ProcessError> {
        let result = self.run().await?;
        if result.success() {
            Ok(result)
        } else {
            Err(ProcessError::Failed {
                program: self.program.clone(),
                result: Box::new(result),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

/// Formats an invocation for display, redacted and minimally quoted.
///
/// The output is for a human to read and understand what ran. It is **not**
/// meant to be pasted into a shell and is not shell-escaped for safety
/// purposes, because nothing here ever reaches a shell.
pub fn format_command(program: &str, args: &[String]) -> String {
    let mut parts = vec![quote_if_needed(program)];
    parts.extend(args.iter().map(|arg| quote_if_needed(&redact::redact(arg))));
    parts.join(" ")
}

fn quote_if_needed(value: &str) -> String {
    if value.is_empty() {
        return "''".to_owned();
    }
    let needs_quotes = value
        .chars()
        .any(|c| c.is_whitespace() || ";&|$`<>()*?[]{}!#'\"\\".contains(c));

    if needs_quotes {
        format!("'{}'", value.replace('\'', r"'\''"))
    } else {
        value.to_owned()
    }
}
