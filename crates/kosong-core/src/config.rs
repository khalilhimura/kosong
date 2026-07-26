//! Non-secret configuration and resumable onboarding state.
//!
//! Two files live in the platform config directory:
//!
//! | File | Holds |
//! |---|---|
//! | `config.toml` | User preferences: editor, API base URL, preview port. |
//! | `onboarding.toml` | How far through the first run the user has got. |
//!
//! **Neither file may ever hold a secret.** Not verification codes, not access
//! or refresh tokens, not document content, not GitHub or Cloudflare
//! credentials. The technical specification states this as a hard rule, and
//! the types here enforce it structurally: there is no field to put a secret
//! in, and unknown keys are *not* preserved on write, so a secret written by
//! hand is dropped rather than propagated.
//!
//! That last point is a deliberate difference from the OKF document, where
//! unknown-key preservation is required. These are `kosong`'s own files, and
//! forgetting an unexpected value is the safer failure here.

use crate::workspace::{self, WorkspaceError};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

/// Environment variable overriding the config directory.
///
/// Exists so tests never touch the real user profile.
pub const CONFIG_DIR_ENV: &str = "KOSONG_CONFIG_DIR";

/// Default port for `kosong preview`.
pub const DEFAULT_PREVIEW_PORT: u16 = 4173;

/// Current schema version of both files.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not work out where to keep settings on this computer")]
    NoConfigDir,

    #[error("`{path}` is not valid TOML: {source}")]
    Malformed {
        path: Utf8PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("could not write settings: {0}")]
    Serialize(#[from] toml::ser::Error),

    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
}

impl ConfigError {
    pub fn repair(&self) -> String {
        match self {
            Self::NoConfigDir => {
                format!("Set {CONFIG_DIR_ENV} to a folder kosong may use for its settings.")
            }
            Self::Malformed { path, .. } => {
                format!("Delete `{path}` and run `kosong doctor`. kosong will rebuild it.")
            }
            Self::Serialize(_) => "This is a bug in kosong. Please report it.".into(),
            Self::Workspace(e) => e.repair(),
        }
    }
}

/// The directory holding `config.toml` and `onboarding.toml`.
///
/// Matches the platform convention named in the technical specification:
/// `~/Library/Application Support/kosong` on macOS, `~/.config/kosong` on
/// Linux. [`CONFIG_DIR_ENV`] overrides both.
pub fn config_dir() -> Result<Utf8PathBuf, ConfigError> {
    if let Some(override_dir) = std::env::var_os(CONFIG_DIR_ENV) {
        let path = Utf8PathBuf::from_path_buf(override_dir.into())
            .map_err(|_| ConfigError::NoConfigDir)?;
        return Ok(path);
    }

    let base = directories_next::BaseDirs::new().ok_or(ConfigError::NoConfigDir)?;
    let dir = Utf8Path::from_path(base.config_dir()).ok_or(ConfigError::NoConfigDir)?;
    Ok(dir.join("kosong"))
}

// ---------------------------------------------------------------------------
// config.toml
// ---------------------------------------------------------------------------

/// User preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,

    /// Editor command, used as the executable name only.
    ///
    /// This is never passed to a shell. `kosong edit` runs it via argv, so a
    /// value like `vim; rm -rf ~` is treated as one absurd executable name
    /// that fails to launch, not as two commands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editor: Option<String>,

    /// Base URL of the remote API. Overridable for local development.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_base_url: Option<String>,

    /// Preferred preview port.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_port: Option<u16>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            editor: None,
            api_base_url: None,
            preview_port: None,
        }
    }
}

impl Config {
    pub fn path() -> Result<Utf8PathBuf, ConfigError> {
        Ok(config_dir()?.join("config.toml"))
    }

    /// Loads config, returning defaults when the file does not exist yet.
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(&Self::path()?)
    }

    pub fn load_from(path: &Utf8Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|source| ConfigError::Malformed {
                path: path.to_owned(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(WorkspaceError::Io {
                path: path.to_owned(),
                source,
            }
            .into()),
        }
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        self.save_to(&Self::path()?)
    }

    pub fn save_to(&self, path: &Utf8Path) -> Result<(), ConfigError> {
        let text = toml::to_string_pretty(self)?;
        workspace::atomic_write(path, text.as_bytes())?;
        Ok(())
    }

    /// The port `preview` should use, honouring the user's preference.
    pub fn preview_port(&self) -> u16 {
        self.preview_port.unwrap_or(DEFAULT_PREVIEW_PORT)
    }

    /// The editor to open, resolved from config then the usual environment.
    ///
    /// Returns the command as written. The caller is responsible for treating
    /// it as an executable name, never a shell string.
    pub fn resolve_editor(&self) -> Option<String> {
        self.editor
            .clone()
            .or_else(|| std::env::var("VISUAL").ok())
            .or_else(|| std::env::var("EDITOR").ok())
            .filter(|value| !value.trim().is_empty())
    }
}

// ---------------------------------------------------------------------------
// onboarding.toml
// ---------------------------------------------------------------------------

/// Resumable first-run progress.
///
/// Every field is a boolean or a path. There is deliberately nowhere to put a
/// token, a code, or document text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Onboarding {
    pub version: u32,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<Utf8PathBuf>,

    pub local_document_created: bool,
    pub preview_completed: bool,
    pub login_completed: bool,
    pub site_initialized: bool,
    pub site_published: bool,
}

impl Default for Onboarding {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            workspace_path: None,
            local_document_created: false,
            preview_completed: false,
            login_completed: false,
            site_initialized: false,
            site_published: false,
        }
    }
}

/// The next thing the user should do.
///
/// Onboarding is a ladder: each rung is a real artifact plus one transferable
/// idea. This maps state to the single next command, so no screen ever shows a
/// control panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextStep {
    CreateDocument,
    Preview,
    Login,
    InitSite,
    Publish,
    Done,
}

impl NextStep {
    /// The command to run, as the user would type it.
    pub fn command(self) -> &'static str {
        match self {
            Self::CreateDocument => "kosong new",
            Self::Preview => "kosong preview",
            Self::Login => "kosong login",
            Self::InitSite => "kosong site init",
            Self::Publish => "kosong site publish",
            Self::Done => "kosong status",
        }
    }

    /// The one idea this step teaches.
    pub fn lesson(self) -> &'static str {
        match self {
            Self::CreateDocument => "A file is a durable object you can inspect and move.",
            Self::Preview => "A local server is a temporary website on your own computer.",
            Self::Login => "Signing in keeps a private copy you can pull back later.",
            Self::InitSite => "Git tracks the history of a folder.",
            Self::Publish => "Deployment moves built files to a host. It is not magic.",
            Self::Done => "Software can describe its current state.",
        }
    }
}

impl Onboarding {
    pub fn path() -> Result<Utf8PathBuf, ConfigError> {
        Ok(config_dir()?.join("onboarding.toml"))
    }

    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(&Self::path()?)
    }

    pub fn load_from(path: &Utf8Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|source| ConfigError::Malformed {
                path: path.to_owned(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(WorkspaceError::Io {
                path: path.to_owned(),
                source,
            }
            .into()),
        }
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        self.save_to(&Self::path()?)
    }

    pub fn save_to(&self, path: &Utf8Path) -> Result<(), ConfigError> {
        let text = toml::to_string_pretty(self)?;
        workspace::atomic_write(path, text.as_bytes())?;
        Ok(())
    }

    /// The next rung of the ladder.
    ///
    /// Login is offered only after preview, because the local artifact must
    /// come before any account. That ordering is the product's core promise.
    pub fn next_step(&self) -> NextStep {
        if !self.local_document_created {
            NextStep::CreateDocument
        } else if !self.preview_completed {
            NextStep::Preview
        } else if !self.login_completed {
            NextStep::Login
        } else if !self.site_initialized {
            NextStep::InitSite
        } else if !self.site_published {
            NextStep::Publish
        } else {
            NextStep::Done
        }
    }
}
