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
//!   logged.

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
            stdout: redact(&String::from_utf8_lossy(&output.stdout)),
            stderr: redact(&String::from_utf8_lossy(&output.stderr)),
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
    parts.extend(args.iter().map(|arg| quote_if_needed(&redact(arg))));
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

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

/// Prefixes that identify a credential beyond reasonable doubt.
const SECRET_PREFIXES: [&str; 15] = [
    "ghp_",        // GitHub personal access token
    "gho_",        // GitHub OAuth token
    "ghu_",        // GitHub user-to-server token
    "ghs_",        // GitHub server-to-server token
    "ghr_",        // GitHub refresh token
    "github_pat_", // GitHub fine-grained token
    "glpat-",      // GitLab
    // Slack, spelled out by token type rather than as the three-character
    // `xox` this used to be. `xox` is a prefix of `xoxo`, which is an ordinary
    // thing to call a blog, and a Pages project named `xoxo-blog` was being
    // blanked out of `wrangler pages project list` as a result. Every Slack
    // token type carries the trailing letter and hyphen, so naming them costs
    // nothing and stops the collision at its source.
    "xoxa-",
    "xoxb-",
    "xoxc-",
    "xoxd-",
    "xoxe-",
    "xoxp-",
    "xoxr-",
    "xoxs-",
];

/// Keys whose value is a credential, in `key=value` or `key: value` form.
const SECRET_KEYS: [&str; 8] = [
    "authorization",
    "bearer",
    "token",
    "api_token",
    "access_token",
    "refresh_token",
    "password",
    "secret",
];

/// The replacement written in place of a secret.
pub const REDACTED: &str = "[redacted]";

/// Removes anything recognisable as a credential from text.
///
/// Applied to every byte of child output before it is stored, displayed, or
/// logged, because `gh` and `wrangler` can echo a token in an error.
///
/// # What this deliberately does not do
///
/// It does not redact every long random-looking string. A 40-character hex
/// run is far more likely to be a commit SHA than a secret, and blanking those
/// would make `git` and `gh` output useless while protecting nothing. The
/// approach is targeted: known credential prefixes, and values that follow a
/// key which names a credential.
pub fn redact(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    // Two passes. The prefix pass catches a credential anywhere, including
    // inside JSON where there is no whitespace to split on. The key/value pass
    // catches secrets that have no recognisable shape of their own and are
    // identifiable only by the word next to them.
    let mut output = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        output.push_str(&redact_key_values(&redact_prefixes(line)));
    }
    output
}

/// Whether a character can be part of a credential.
fn is_secret_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Replaces anything starting with a known credential prefix.
///
/// Scans the whole string rather than whitespace-separated words, so a token
/// embedded in JSON such as `{"token":"ghp_..."}` is still caught. Punctuation
/// is what makes that work: `"`, `=`, `:` and whitespace all end a word, so a
/// credential wrapped in any of them still begins one.
///
/// # A credential begins a word
///
/// The scan starts at every byte, so without a boundary check a prefix landing
/// mid-word took the rest of the word with it — `the-ghp_naming-convention`
/// became `the-[redacted]`. Over-redaction is not a safe failure here: this
/// output is parsed as well as displayed, and `project_exists` reading a
/// blanked-out project name concludes the project is absent and has kosong
/// create one that already exists.
fn redact_prefixes(text: &str) -> String {
    let lowered = text.to_ascii_lowercase();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;

    while index < text.len() {
        // `index` is always a character boundary: the no-match arm below
        // advances by whole characters, and a prefix is only ever consumed
        // together with the credential body after it.
        let begins_a_word = text[..index]
            .chars()
            .next_back()
            .is_none_or(|c| !is_secret_char(c));

        let matched = SECRET_PREFIXES
            .iter()
            .filter(|_| begins_a_word)
            .find(|prefix| lowered[index..].starts_with(*prefix));

        match matched {
            Some(prefix) => {
                // Consume the prefix and the credential body that follows it.
                let mut end = index + prefix.len();
                while let Some(c) = text[end..].chars().next() {
                    if is_secret_char(c) {
                        end += c.len_utf8();
                    } else {
                        break;
                    }
                }
                output.push_str(REDACTED);
                index = end;
            }
            None => {
                let c = text[index..].chars().next().expect("in bounds");
                output.push(c);
                index += c.len_utf8();
            }
        }
    }
    output
}

/// Replaces values identified by a neighbouring or attached credential key.
///
/// Handles `token=abc`, `token: abc`, `Authorization: Bearer abc`, and
/// `--api-token abc`. The label may be attached to the value or be the
/// preceding word, so both are handled in one pass over the line.
fn redact_key_values(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut remainder = line;
    let mut previous_was_label = false;

    while !remainder.is_empty() {
        let boundary = remainder
            .find(char::is_whitespace)
            .unwrap_or(remainder.len());
        let (token, rest) = remainder.split_at(boundary);

        if previous_was_label && !token.is_empty() && token != REDACTED {
            if names_a_credential(token) {
                // `Authorization: Bearer abc` — the word after the label is
                // another label, so the secret is one further along. Treating
                // "Bearer" as the value would leave the real token exposed.
                output.push_str(token);
                previous_was_label = true;
            } else {
                // The whole word is the secret.
                output.push_str(REDACTED);
                previous_was_label = false;
            }
        } else {
            let (rendered, is_label) = redact_token(token);
            output.push_str(&rendered);
            previous_was_label = is_label;
        }

        let separator_end = rest
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(rest.len());
        output.push_str(&rest[..separator_end]);
        remainder = &rest[separator_end..];
    }
    output
}

/// Redacts an attached `key=value`, and reports whether the token is a bare
/// credential label whose value is the next word.
fn redact_token(token: &str) -> (String, bool) {
    if token.is_empty() {
        return (String::new(), false);
    }

    // A bare label: `Authorization:`, `Bearer`, `--api-token`.
    if names_a_credential(token) {
        return (token.to_owned(), true);
    }

    for separator in ['=', ':'] {
        if let Some((key, value)) = token.split_once(separator) {
            let trimmed_value = value.trim_matches(|c: char| "\"'".contains(c));
            if trimmed_value.is_empty() {
                continue;
            }
            if names_a_credential(key) {
                return (token.replace(trimmed_value, REDACTED), false);
            }
        }
    }
    (token.to_owned(), false)
}

/// Whether a word names a credential, ignoring decoration.
///
/// Strips quotes, braces, leading dashes, and a trailing colon, so
/// `{"api_token"`, `--api-token`, and `Authorization:` all reduce to the bare
/// key before comparison.
fn names_a_credential(word: &str) -> bool {
    let cleaned: String = word
        .trim_matches(|c: char| !c.is_ascii_alphanumeric())
        .to_ascii_lowercase()
        .replace('-', "_");

    if cleaned.is_empty() {
        return false;
    }
    SECRET_KEYS
        .iter()
        .any(|key| cleaned == key.replace('-', "_") || cleaned.ends_with(&format!("_{key}")))
}
