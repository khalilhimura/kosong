//! `kosong site init`, `publish`, and `rollback`.
//!
//! Every mutating step follows §12.4: validate, build a typed plan, disclose
//! executable and argv and working directory and files and remote effect, stop
//! on `--dry-run`, and require confirmation unless `--yes`.

pub mod init;
pub mod publish;
pub mod rollback;

// Re-export the command entry points so they're reachable at `commands::site::init` etc.
pub use init::init;
pub use publish::publish;
pub use rollback::rollback;

// Re-export shared types so submodules can use `super::*`.
pub(crate) use super::Context;
pub(crate) use super::provider::map_process_error;
pub(crate) use super::provider::map_provider_error;
pub(crate) use crate::exit::{CliError, CliResult};
pub(crate) use crate::ui::{Mark, Ui};
pub(crate) use camino::{Utf8Path, Utf8PathBuf};
pub(crate) use kosong_core::process::ProcessResult;
pub(crate) use kosong_core::providers::Operation;
pub(crate) use kosong_core::{SiteError, Workspace};

/// How a mutating step should be handled.
#[derive(Debug, Clone, Copy)]
pub struct Approval {
    pub dry_run: bool,
    pub assume_yes: bool,
}

/// Maps a `SiteError` to a `CliError` with the right exit code and repair text.
pub(crate) fn map_site_error(error: SiteError) -> CliError {
    let repair = error.repair();
    CliError::usage("SITE_ERROR", error.to_string()).with_repair(repair)
}

/// Runs an operation, honouring disclosure, dry run, and confirmation.
///
/// Returns `Ok(None)` when a dry run stopped before doing anything, so a
/// caller can distinguish "did not run" from "ran and produced nothing".
pub(crate) fn perform(
    ui: Ui,
    operation: &dyn Operation,
    cwd: &Utf8Path,
    approval: Approval,
) -> CliResult<Option<ProcessResult>> {
    // `cwd` is the site folder for every caller in this file, which is where
    // `npm install` runs and so where a project-local install lands. `None` —
    // the usual answer — leaves the invocation exactly as it was before local
    // resolution existed.
    let resolved = crate::tools::resolved_program(operation.program(), cwd);

    let plan = match &resolved {
        Some(path) => operation.plan(cwd).found_at(path.clone()),
        None => operation.plan(cwd),
    };

    if plan.mutating {
        if plan.remote_effect.is_some() || approval.dry_run {
            for line in plan.disclosure() {
                ui.say(line);
            }
            ui.blank();
        } else {
            ui.lesson(format!("  {}", plan.display_command()));
        }

        if approval.dry_run {
            ui.status(Mark::Info, "dry run", "nothing was changed");
            return Ok(None);
        }

        if plan.remote_effect.is_some() && !approval.assume_yes && !ui.confirm("Go ahead?", false) {
            return Err(
                CliError::usage("CANCELLED", "stopped before making any changes")
                    .with_repair("Nothing was changed. Run the command again when you are ready."),
            );
        }
    } else if approval.dry_run {
        ui.lesson(format!("  (checking) {}", plan.display_command()));
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::internal("RUNTIME_FAILED", e.to_string()))?;

    let mut command = operation.command(cwd);
    if let Some(path) = resolved {
        command = command.found_at(path);
    }

    let result = runtime.block_on(command.run()).map_err(map_process_error)?;

    Ok(Some(result))
}

/// Runs an operation and fails if it did.
pub(crate) fn perform_checked(
    ui: Ui,
    operation: &dyn Operation,
    cwd: &Utf8Path,
    approval: Approval,
) -> CliResult<Option<ProcessResult>> {
    let Some(result) = perform(ui, operation, cwd, approval)? else {
        return Ok(None);
    };
    if result.success() {
        Ok(Some(result))
    } else {
        Err(
            CliError::provider("TOOL_FAILED", format!("`{}` failed", result.executable))
                .with_repair(result.combined_output()),
        )
    }
}

pub(crate) fn locate_site(workspace: &Workspace) -> CliResult<Utf8PathBuf> {
    kosong_core::site::discover(workspace).ok_or_else(|| map_site_error(SiteError::NotInitialized))
}

/// Whether `git` is on this machine's PATH.
pub(crate) fn git_is_available() -> bool {
    crate::tools::find_executable(kosong_core::providers::git::PROGRAM).is_some()
}

/// The owned paths git will accept: those on disk, plus those it still tracks.
pub(crate) fn stageable(
    ui: Ui,
    site_root: &Utf8Path,
    paths: Vec<Utf8PathBuf>,
    approval: Approval,
) -> CliResult<Vec<Utf8PathBuf>> {
    use kosong_core::providers::git::{self, GitOperation};

    let tracked = perform(
        ui,
        &GitOperation::list_tracked(paths.clone()),
        site_root,
        approval,
    )?
    .filter(|result| result.success())
    .map(|result| result.stdout)
    .unwrap_or_default();

    Ok(paths
        .into_iter()
        .filter(|path| site_root.join(path).exists() || git::tracks(&tracked, path))
        .collect())
}
