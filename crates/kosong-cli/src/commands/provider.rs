//! `kosong gh` and `kosong cf` — the allowlisted provider commands.
//!
//! §5.1 lists `kosong gh <allowlisted-subcommand>` and `kosong cf
//! <allowlisted-subcommand>`, while §6.2 puts arbitrary pass-through out of
//! scope. So these are not pass-throughs: each subcommand here maps to one
//! variant of a closed enum, and there is no path from user input to an
//! argument vector.
//!
//! Anything a user wants that is not listed can still be done by running `gh`
//! or `wrangler` themselves. That is the point of the ownership promise — the
//! tools are theirs, and `kosong` is not a gatekeeper.

use super::Context;
use crate::exit::{CliError, CliResult, Exit};
use crate::ui::Mark;
use kosong_core::process::ProcessError;
use kosong_core::providers::Operation;
use kosong_core::providers::cloudflare::{self, CloudflareOperation};
use kosong_core::providers::github::{self, GitHubOperation};

/// Read-only GitHub subcommands offered by `kosong gh`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum GhSubcommand {
    /// Whether GitHub CLI is signed in.
    Auth,
    /// Show the repository connected to this folder.
    Repo,
}

/// Read-only Cloudflare subcommands offered by `kosong cf`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum CfSubcommand {
    /// Which Cloudflare account is signed in.
    Whoami,
    /// Your Pages projects.
    Projects,
    /// Past deployments of this site.
    Deployments,
}

pub fn run_gh(context: &Context, subcommand: GhSubcommand, name: Option<String>) -> CliResult<()> {
    let ui = context.ui;
    let workspace = context.workspace_or_here()?;

    let operation = match subcommand {
        GhSubcommand::Auth => GitHubOperation::AuthStatus,
        GhSubcommand::Repo => {
            let repo = name.ok_or_else(|| {
                CliError::usage("NO_REPO", "which repository should kosong look up?")
                    .with_repair("Run: kosong gh repo owner/name")
            })?;
            GitHubOperation::repo_view(&repo).map_err(map_provider_error)?
        }
    };

    let result = execute(context, &operation, workspace.root())?;

    if subcommand == GhSubcommand::Auth {
        let state = github::interpret_auth_status(result.exit_code, &result.combined_output());
        report_auth(ui, "GitHub CLI", github::gh_repair(state));
        return Ok(());
    }

    ui.always(result.combined_output());
    Ok(())
}

pub fn run_cf(context: &Context, subcommand: CfSubcommand) -> CliResult<()> {
    let ui = context.ui;
    let workspace = context.workspace_or_here()?;

    let operation = match subcommand {
        CfSubcommand::Whoami => CloudflareOperation::WhoAmI,
        CfSubcommand::Projects => CloudflareOperation::PagesProjectList,
        CfSubcommand::Deployments => {
            // The project name comes from site state, which lands with
            // `site init` in the next phase.
            return Err(
                CliError::usage("NO_SITE", "this folder has no published site yet")
                    .with_repair("Run `kosong site init` first."),
            );
        }
    };

    let result = execute(context, &operation, workspace.root())?;

    if subcommand == CfSubcommand::Whoami {
        let state = cloudflare::interpret_whoami(result.exit_code, &result.combined_output());
        report_auth(ui, "Wrangler", cloudflare::wrangler_repair(state));
        return Ok(());
    }

    ui.always(result.combined_output());
    Ok(())
}

/// Runs an operation, honouring `--dry-run` and confirmation.
///
/// Read-only operations run without asking. A mutating one follows §12.4:
/// disclose, stop on `--dry-run`, then confirm unless `--yes`.
fn execute(
    context: &Context,
    operation: &dyn Operation,
    cwd: &camino::Utf8Path,
) -> CliResult<kosong_core::process::ProcessResult> {
    let ui = context.ui;

    // A project-local install lives in the *site* folder, which can sit one
    // level below the workspace root these commands run from. `cwd` is left
    // alone: which wrangler runs and where it runs are separate decisions, and
    // only the first one is being changed.
    let resolved = site_root(context)
        .as_deref()
        .and_then(|root| crate::tools::resolved_program(operation.program(), root));

    let plan = match &resolved {
        Some(path) => operation.plan(cwd).found_at(path.clone()),
        None => operation.plan(cwd),
    };

    if plan.mutating {
        for line in plan.disclosure() {
            ui.say(line);
        }
        ui.blank();
    }

    let mut command = operation.command(cwd);
    if let Some(path) = resolved {
        command = command.found_at(path);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::internal("RUNTIME_FAILED", e.to_string()))?;

    runtime.block_on(command.run()).map_err(map_process_error)
}

/// The site folder, when this workspace has one.
///
/// Where a project-local tool install lives. Absent for a workspace that has not
/// run `site init`, which simply means there is nothing local to prefer.
fn site_root(context: &Context) -> Option<camino::Utf8PathBuf> {
    let workspace = context.workspace_or_here().ok()?;
    kosong_core::site::discover(&workspace)
}

fn report_auth(ui: crate::ui::Ui, tool: &str, repair: Option<&'static str>) {
    match repair {
        None => ui.status(Mark::Good, format!("{tool} is signed in"), ""),
        Some(repair) => {
            ui.status(Mark::Warn, format!("{tool} is not ready"), "");
            ui.blank();
            for line in repair.lines() {
                ui.say(line);
            }
        }
    }
}

/// External tool failures map to exit code 4, per §12.4.
pub fn map_process_error(error: ProcessError) -> CliError {
    let repair = error.repair();
    let code = match &error {
        ProcessError::NotFound { .. } => "TOOL_NOT_INSTALLED",
        ProcessError::Timeout { .. } => "TOOL_TIMEOUT",
        ProcessError::Spawn { .. } => "TOOL_SPAWN_FAILED",
        ProcessError::Failed { .. } => "TOOL_FAILED",
    };
    CliError::new(Exit::Provider, code, error.to_string()).with_repair(repair)
}

pub fn map_provider_error(error: kosong_core::ProviderError) -> CliError {
    let repair = error.repair();
    CliError::usage("INVALID_NAME", error.to_string()).with_repair(repair)
}
