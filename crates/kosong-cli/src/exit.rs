//! Exit codes and the error type that carries them.
//!
//! Technical specification §5.3 fixes the numbers, so scripts can depend on
//! them. Each error also carries a stable machine `code` for internal use and,
//! wherever one exists, a plain-language repair action — the product rule is
//! that a failure should leave the user knowing what to do next.

use std::fmt;

/// Process exit codes. The numbers are contractual; do not renumber.
///
/// The full set is defined here even though the local commands cannot yet
/// produce `Auth` or `Network`. These are a published interface that scripts
/// depend on, so the enum matches the specification rather than whichever
/// subset happens to be reachable in the current phase.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Exit {
    Success = 0,
    /// Invalid input, or a local precondition that is not met.
    Usage = 2,
    /// Authentication or session problem.
    Auth = 3,
    /// An external tool or provider prerequisite is missing.
    Provider = 4,
    /// Network or remote service problem.
    Network = 5,
    /// Something went wrong that should not have.
    Internal = 10,
}

impl Exit {
    pub fn code(self) -> i32 {
        self as i32
    }
}

/// A failure to report to the user and exit on.
#[derive(Debug)]
pub struct CliError {
    pub exit: Exit,
    /// Stable machine-readable code, e.g. `NO_DOCUMENT`. Never shown by default.
    ///
    /// §5.3 requires errors to carry a stable internal code. Read by tests
    /// today and by structured logging once it lands.
    #[allow(dead_code)]
    pub code: &'static str,
    /// What went wrong, in plain language.
    pub message: String,
    /// What to do about it.
    pub repair: Option<String>,
}

impl CliError {
    pub fn new(exit: Exit, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            exit,
            code,
            message: message.into(),
            repair: None,
        }
    }

    /// Attaches a next action.
    pub fn with_repair(mut self, repair: impl Into<String>) -> Self {
        let repair = repair.into();
        if !repair.trim().is_empty() {
            self.repair = Some(repair);
        }
        self
    }

    pub fn usage(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Exit::Usage, code, message)
    }

    /// A provider — GitHub, Cloudflare, npm, git — refused or could not run.
    pub fn provider(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Exit::Provider, code, message)
    }

    pub fn internal(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Exit::Internal, code, message)
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

pub type CliResult<T> = Result<T, CliError>;

// ---------------------------------------------------------------------------
// Conversions from core errors
// ---------------------------------------------------------------------------
//
// Core errors already carry a repair suggestion. These conversions keep it and
// map the failure onto the right exit code, so no call site has to remember.

impl From<kosong_core::OkfError> for CliError {
    fn from(error: kosong_core::OkfError) -> Self {
        let repair = error.repair();
        Self::usage("INVALID_OKF", error.to_string()).with_repair(repair)
    }
}

impl From<kosong_core::WorkspaceError> for CliError {
    fn from(error: kosong_core::WorkspaceError) -> Self {
        use kosong_core::WorkspaceError as W;

        let repair = error.repair();
        let code = match error {
            W::NotFound => "NO_WORKSPACE",
            W::NotADirectory { .. } => "NOT_A_DIRECTORY",
            W::IsSymlink { .. } => "SYMLINK_REFUSED",
            W::OutsideWorkspace { .. } => "OUTSIDE_WORKSPACE",
            W::NonUtf8Path { .. } => "NON_UTF8_PATH",
            W::ReservedFilename { .. } => "RESERVED_FILENAME",
            W::TooLarge { .. } => "DOCUMENT_TOO_LARGE",
            W::AlreadyExists { .. } => "DOCUMENT_EXISTS",
            W::Io { .. } => "IO_ERROR",
            W::Okf(_) => "INVALID_OKF",
        };
        Self::usage(code, error.to_string()).with_repair(repair)
    }
}

impl From<kosong_core::ConfigError> for CliError {
    fn from(error: kosong_core::ConfigError) -> Self {
        let repair = error.repair();
        Self::usage("CONFIG_ERROR", error.to_string()).with_repair(repair)
    }
}

impl From<kosong_core::PreviewError> for CliError {
    fn from(error: kosong_core::PreviewError) -> Self {
        let repair = error.repair();
        Self::usage("PREVIEW_ERROR", error.to_string()).with_repair(repair)
    }
}
