//! Cloudflare Pages, through `wrangler`.
//!
//! Permitted operations, per technical specification §12.2: `whoami`,
//! `pages project list`, `pages project create`, `pages deploy`,
//! `pages deployment list`.
//!
//! # Why `pages project create` is here
//!
//! It was not in the original set, and its absence made `kosong site publish`
//! impossible to complete for anyone publishing for the first time: `wrangler
//! pages deploy --project-name X` fails outright when `X` does not exist, and
//! tells the user to run a command kosong did not offer. Found by the first
//! substantive live smoke run, which is what that workflow exists for.
//!
//! Verified against Wrangler 4.114: `wrangler pages project create
//! <project-name> [--production-branch <branch>]`.
//!
//! # There is no rollback operation here, and that is deliberate
//!
//! §12.2 also anticipates an "exact rollback operation validated against
//! current official documentation", and §13.4 requires checking that syntax
//! immediately before implementation rather than assuming it.
//!
//! That check was done against Wrangler 4.114 and the current Cloudflare
//! documentation. **Wrangler has no Pages rollback command.** The
//! `pages deployment` subcommands are `list`, `create`, `tail`, and `delete`.
//! The top-level `wrangler rollback` applies to Workers, not Pages. Cloudflare
//! documents Pages rollback as a dashboard action.
//!
//! Rolling back through Cloudflare's REST API would need an API token in
//! `kosong`'s hands, which architectural rule 2 in §2 forbids: provider
//! credentials stay in `wrangler`'s own store and the CLI never receives them.
//!
//! So no rollback variant is offered. Inventing one that quietly did something
//! else — deleting the newer deployment, say — would be worse than not having
//! it. See [`rollback_guidance`] for what `kosong` tells the user instead.

use super::{Operation, ProviderError, validate_name};
use camino::{Utf8Path, Utf8PathBuf};
use std::time::Duration;

/// The executable. Fixed, never derived from input.
pub const PROGRAM: &str = "wrangler";

/// A permitted Cloudflare operation.
///
/// Closed by construction, like [`super::github::GitHubOperation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudflareOperation {
    /// Which Cloudflare account `wrangler` is signed in to.
    WhoAmI,

    /// The Pages projects on that account.
    PagesProjectList,

    /// Create a Pages project to publish into.
    PagesProjectCreate {
        project: String,
        /// Which branch Cloudflare treats as production. Always stated rather
        /// than left to wrangler, which would otherwise ask — and kosong runs
        /// wrangler with no terminal to answer from.
        production_branch: Option<String>,
    },

    /// The deployment history for one project.
    PagesDeploymentList { project: String },

    /// Upload a built directory as a new deployment.
    PagesDeploy {
        project: String,
        /// Directory of built output, relative to the working directory.
        directory: Utf8PathBuf,
        /// Branch name recorded against the deployment.
        branch: Option<String>,
    },
}

impl CloudflareOperation {
    pub fn project_create(
        project: &str,
        production_branch: Option<&str>,
    ) -> Result<Self, ProviderError> {
        let production_branch = match production_branch {
            Some(value) => Some(validate_name("branch", value)?),
            None => None,
        };
        Ok(Self::PagesProjectCreate {
            project: validate_name("project", project)?,
            production_branch,
        })
    }

    pub fn deployment_list(project: &str) -> Result<Self, ProviderError> {
        Ok(Self::PagesDeploymentList {
            project: validate_name("project", project)?,
        })
    }

    pub fn deploy(
        project: &str,
        directory: &Utf8Path,
        branch: Option<&str>,
    ) -> Result<Self, ProviderError> {
        let branch = match branch {
            Some(value) => Some(validate_name("branch", value)?),
            None => None,
        };
        Ok(Self::PagesDeploy {
            project: validate_name("project", project)?,
            directory: directory.to_owned(),
            branch,
        })
    }
}

impl Operation for CloudflareOperation {
    fn program(&self) -> &'static str {
        PROGRAM
    }

    fn args(&self) -> Vec<String> {
        match self {
            Self::WhoAmI => vec!["whoami".into()],

            Self::PagesProjectList => {
                vec!["pages".into(), "project".into(), "list".into()]
            }

            Self::PagesProjectCreate {
                project,
                production_branch,
            } => {
                let mut args = vec![
                    "pages".into(),
                    "project".into(),
                    "create".into(),
                    project.clone(),
                ];
                if let Some(branch) = production_branch {
                    args.push("--production-branch".into());
                    args.push(branch.clone());
                }
                args
            }

            Self::PagesDeploymentList { project } => vec![
                "pages".into(),
                "deployment".into(),
                "list".into(),
                "--project-name".into(),
                project.clone(),
            ],

            Self::PagesDeploy {
                project,
                directory,
                branch,
            } => {
                let mut args = vec![
                    "pages".into(),
                    "deploy".into(),
                    directory.to_string(),
                    "--project-name".into(),
                    project.clone(),
                ];
                if let Some(branch) = branch {
                    args.push("--branch".into());
                    args.push(branch.clone());
                }
                args
            }
        }
    }

    fn mutating(&self) -> bool {
        match self {
            Self::WhoAmI | Self::PagesProjectList | Self::PagesDeploymentList { .. } => false,
            Self::PagesProjectCreate { .. } | Self::PagesDeploy { .. } => true,
        }
    }

    fn summary(&self) -> String {
        match self {
            Self::WhoAmI => "Check which Cloudflare account is signed in.".into(),
            Self::PagesProjectList => "List your Cloudflare Pages projects.".into(),
            Self::PagesProjectCreate { project, .. } => {
                format!("Make a place on Cloudflare for `{project}` to live.")
            }
            Self::PagesDeploymentList { project } => {
                format!("List past deployments of `{project}`.")
            }
            Self::PagesDeploy { project, .. } => {
                format!("Publish your built page to `{project}`.")
            }
        }
    }

    fn remote_effect(&self) -> Option<String> {
        match self {
            Self::WhoAmI | Self::PagesProjectList | Self::PagesDeploymentList { .. } => None,
            Self::PagesProjectCreate { project, .. } => Some(format!(
                "creates an empty Cloudflare Pages project called `{project}` on your account. \
                 Nothing is published to it by this step, and the address it reserves is public"
            )),
            Self::PagesDeploy { project, .. } => Some(format!(
                "uploads your built files to Cloudflare and makes them the live version of \
                 `{project}`, visible to anyone with the address"
            )),
        }
    }

    fn files(&self) -> Vec<Utf8PathBuf> {
        match self {
            Self::PagesDeploy { directory, .. } => vec![directory.clone()],
            _ => Vec::new(),
        }
    }

    fn timeout(&self) -> Duration {
        match self {
            Self::PagesDeploy { .. } => crate::process::LONG_TIMEOUT,
            _ => crate::process::QUERY_TIMEOUT,
        }
    }
}

/// What to tell a user who asks to roll back.
///
/// Honest about the boundary rather than pretending to a capability that does
/// not exist. Both routes are real and both leave the user in control.
pub fn rollback_guidance(project: &str) -> String {
    format!(
        "Cloudflare does not offer a way to roll back a Pages site from the command line, \
         so kosong cannot do it for you without holding your Cloudflare password, which it \
         is designed never to do.\n\
         \n\
         Two ways forward:\n\
         \n\
         1. Roll back in your browser, which is instant:\n\
         \x20  https://dash.cloudflare.com  →  Workers & Pages  →  {project}  →  Deployments\n\
         \x20  Find the version you want, open its menu, choose \"Rollback to this deployment\".\n\
         \n\
         2. Put the old text back and publish again:\n\
         \x20  kosong edit\n\
         \x20  kosong site publish\n\
         \n\
         To see what has been published:\n\
         \x20  kosong cf deployments"
    )
}

/// Reads `wrangler whoami` output to decide whether the user is signed in.
pub fn interpret_whoami(exit_code: Option<i32>, output: &str) -> AuthState {
    if exit_code == Some(0) {
        // wrangler exits 0 while reporting that nobody is signed in, so the
        // text has to be consulted rather than trusting the code alone.
        if output.contains("not authenticated") || output.contains("You are not logged in") {
            return AuthState::SignedOut;
        }
        return AuthState::SignedIn;
    }
    AuthState::Unknown
}

/// Whether `wrangler` is usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthState {
    SignedIn,
    SignedOut,
    Unknown,
}

impl AuthState {
    /// A plain-language next action, in the CLI's voice.
    pub fn repair(self) -> Option<&'static str> {
        match self {
            Self::SignedIn => None,
            Self::SignedOut => Some(
                "Wrangler is installed, but it is not signed in to Cloudflare yet.\n\
                 Run: wrangler login\n\
                 Then come back and run: kosong doctor",
            ),
            Self::Unknown => Some(
                "Wrangler could not tell kosong whether it is signed in.\n\
                 Run: wrangler whoami",
            ),
        }
    }
}
