//! Command implementations.

pub mod account;
pub mod doctor;
pub mod edit;
pub mod login;
pub mod new;
pub mod preview;
pub mod provider;
pub mod show;
pub mod site;
pub mod start;
pub mod status;
pub mod mcp;
pub mod sync;

use crate::exit::{CliError, CliResult};
use crate::ui::Ui;
use camino::Utf8PathBuf;
use kosong_core::{Config, Onboarding, Workspace};
use std::sync::Arc;

/// Everything a command needs that is not one of its own flags.
#[derive(Clone)]
pub struct Context {
    pub ui: Ui,
    /// Folder to work in: `--workspace` if given, otherwise the current folder.
    pub here: Utf8PathBuf,
    /// Shared single-threaded runtime, lazily initialised on first use.
    ///
    /// Many commands (`site`, `provider`, `doctor`) need to drive async provider
    /// calls from a synchronous context. Creating a fresh runtime per call is
    /// wasteful; the Context holds one for reuse within a single command run.
    runtime: Arc<tokio::runtime::Runtime>,
}

impl Context {
    pub fn new(ui: Ui, workspace: Option<Utf8PathBuf>) -> CliResult<Self> {
        let here = match workspace {
            Some(path) => path,
            None => {
                let cwd = std::env::current_dir().map_err(|e| {
                    CliError::internal("NO_CWD", format!("could not read the current folder: {e}"))
                })?;
                Utf8PathBuf::from_path_buf(cwd).map_err(|path| {
                    CliError::usage(
                        "NON_UTF8_CWD",
                        format!(
                            "the current folder name is not valid UTF-8: {}",
                            path.display()
                        ),
                    )
                    .with_repair("Run kosong from a folder whose name uses ordinary characters.")
                })?
            }
        };

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| {
                CliError::internal("RUNTIME_FAILED", format!("could not start async runtime: {e}"))
            })?;

        Ok(Self {
            ui,
            here,
            runtime: Arc::new(runtime),
        })
    }

    /// Finds an existing workspace, walking up from [`here`](Self::here).
    ///
    /// Used by commands that need a document to already exist.
    pub fn workspace(&self) -> CliResult<Workspace> {
        Ok(Workspace::discover(&self.here)?)
    }

    /// Opens [`here`](Self::here) as a workspace without requiring a document.
    ///
    /// Used by `new` and `start`, which create the document.
    pub fn workspace_here(&self) -> CliResult<Workspace> {
        Ok(Workspace::open(&self.here)?)
    }

    /// Finds an existing workspace, or falls back to the current folder.
    ///
    /// For commands that may legitimately run before any document exists.
    /// `sync` is the motivating case: pulling onto a second machine starts
    /// with an empty folder, so requiring a document first would make the
    /// feature impossible to use for the one thing it is for.
    pub fn workspace_or_here(&self) -> CliResult<Workspace> {
        match Workspace::discover(&self.here) {
            Ok(workspace) => Ok(workspace),
            Err(_) => self.workspace_here(),
        }
    }

    pub fn config(&self) -> CliResult<Config> {
        Ok(Config::load()?)
    }

    /// The API base URL: `KOSONG_API_URL`, then config, then the default.
    ///
    /// The environment override comes first so a developer can point at a
    /// local Worker without editing their real settings file.
    pub fn api_base_url(&self) -> CliResult<String> {
        if let Ok(url) = std::env::var("KOSONG_API_URL") {
            if !url.trim().is_empty() {
                return Ok(url.trim().to_owned());
            }
        }
        Ok(self
            .config()?
            .api_base_url
            .unwrap_or_else(|| kosong_core::api::DEFAULT_BASE_URL.to_owned()))
    }

    pub fn onboarding(&self) -> CliResult<Onboarding> {
        Ok(Onboarding::load()?)
    }

    /// Saves onboarding progress.
    ///
    /// Failing to record progress must not fail the command the user actually
    /// asked for, so this warns rather than returning an error.
    pub fn save_onboarding(&self, state: &Onboarding) {
        if let Err(e) = state.save() {
            self.ui
                .warn(format!("could not remember your progress: {e}"));
        }
    }

    /// Runs a future on the shared runtime.
    ///
    /// All async provider calls in the CLI should go through this method rather
    /// than creating their own [`tokio::runtime::Runtime`].
    pub fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }
}
