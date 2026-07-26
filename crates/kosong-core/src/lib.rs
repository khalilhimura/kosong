//! Local document core for `kosong`.
//!
//! This crate holds everything that works without a network: Open Knowledge
//! Format parsing and validation, workspace safety, configuration, rendering,
//! and local preview. Per the architectural rules in the technical
//! specification, it must not depend on the remote API or on any cloud
//! provider.
//!
//! The document format is Google Cloud's [Open Knowledge Format][okf] v0.1.
//! `kosong` profiles it rather than defining a format of its own; see
//! `spec/okf-profile-v1.md`.
//!
//! [okf]: https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md

pub mod api;
pub mod config;
pub mod okf;
pub mod preview;
pub mod process;
pub mod providers;
pub mod render;
pub mod session;
pub mod site;
pub mod sync;
pub mod workspace;

pub use api::{ApiClient, ApiError};
pub use config::{Config, ConfigError, NextStep, Onboarding};
pub use okf::{Document, KosongMeta, LineEnding, OkfError, Visibility};
pub use preview::{Preview, PreviewError};
pub use process::{ProcessError, ProcessResult, SafeCommand};
pub use providers::{Operation, OperationPlan, ProviderError};
pub use render::Rendered;
pub use session::{Backend, SessionError, SessionStore};
pub use site::{SiteError, SiteState};
pub use sync::{SyncAction, SyncState};
pub use workspace::{Workspace, WorkspaceError};
