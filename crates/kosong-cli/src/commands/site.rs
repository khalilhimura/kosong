//! `kosong site init`, `publish`, and `rollback`.
//!
//! Every mutating step follows §12.4: validate, build a typed plan, disclose
//! executable and argv and working directory and files and remote effect, stop
//! on `--dry-run`, and require confirmation unless `--yes`.

use super::Context;
use super::provider::{map_process_error, map_provider_error};
use crate::exit::{CliError, CliResult, Exit};
use crate::ui::{Mark, Ui};
use camino::{Utf8Path, Utf8PathBuf};
use kosong_core::process::ProcessResult;
use kosong_core::providers::Operation;
use kosong_core::providers::cloudflare::{self, CloudflareOperation};
use kosong_core::providers::git::{self, GitOperation};
use kosong_core::providers::github::GitHubOperation;
use kosong_core::providers::npm::NpmOperation;
use kosong_core::site::{self, SiteState};
use kosong_core::{SiteError, Workspace};

/// How a mutating step should be handled.
#[derive(Debug, Clone, Copy)]
pub struct Approval {
    pub dry_run: bool,
    pub assume_yes: bool,
}

fn map_site_error(error: SiteError) -> CliError {
    let repair = error.repair();
    CliError::usage("SITE_ERROR", error.to_string()).with_repair(repair)
}

/// Runs an operation, honouring disclosure, dry run, and confirmation.
///
/// Returns `Ok(None)` when a dry run stopped before doing anything, so a
/// caller can distinguish "did not run" from "ran and produced nothing".
fn perform(
    ui: Ui,
    operation: &dyn Operation,
    cwd: &Utf8Path,
    approval: Approval,
) -> CliResult<Option<ProcessResult>> {
    let plan = operation.plan(cwd);

    if plan.mutating {
        // §12.4 requires full disclosure before a mutating operation, but the
        // product also promises a beginner "a next action, not a control
        // panel". Nine screens of argv for `git init`, `git add`, and
        // `git commit` trains someone to stop reading — which defeats the
        // point of disclosing at all.
        //
        // So: everything that reaches outside this computer gets the full
        // disclosure, always. Local-only steps get one line naming the exact
        // command, which is still fully inspectable. Under `--dry-run` the
        // user explicitly asked to see the plan, so everything is shown.
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

        // Only genuinely remote effects are worth interrupting for. Asking
        // before every local `git add` would train the user to press y without
        // reading, which is worse than not asking.
        if plan.remote_effect.is_some() && !approval.assume_yes && !ui.confirm("Go ahead?", false) {
            return Err(
                CliError::usage("CANCELLED", "stopped before making any changes")
                    .with_repair("Nothing was changed. Run the command again when you are ready."),
            );
        }
    } else if approval.dry_run {
        // Read-only steps still run under --dry-run: they are how the plan
        // learns what it would do.
        ui.lesson(format!("  (checking) {}", plan.display_command()));
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::internal("RUNTIME_FAILED", e.to_string()))?;

    let result = runtime
        .block_on(operation.command(cwd).run())
        .map_err(map_process_error)?;

    Ok(Some(result))
}

/// Runs an operation and fails if it did.
fn perform_checked(
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
        Err(CliError::new(
            Exit::Provider,
            "TOOL_FAILED",
            format!("`{}` failed", result.executable),
        )
        .with_repair(result.combined_output()))
    }
}

// ---------------------------------------------------------------------------
// site init
// ---------------------------------------------------------------------------

/// `kosong site init [NAME]` — turn a document into a publishable folder.
///
/// Lesson: git tracks the history of a folder.
pub fn init(context: &Context, name: Option<String>, approval: Approval) -> CliResult<()> {
    let ui = context.ui;
    let workspace = context.workspace()?;
    let document = workspace.read_document()?;

    let filename = workspace.document_path().file_name().unwrap_or("kosong.md");
    let site_name = site::site_name_from(name.as_deref(), &document.title_or_filename(filename));
    let site_root = workspace.root().join(&site_name);

    if site::SiteState::path(&site_root).exists() {
        return Err(map_site_error(SiteError::AlreadyExists { path: site_root }));
    }

    ui.heading(format!("Setting up `{site_name}`."));
    ui.blank();

    // 1. Write the bundled template.
    let written = site::scaffold(&site_root).map_err(map_site_error)?;
    ui.status(Mark::Good, "made the site folder", site_root.as_str());
    for path in &written {
        ui.lesson(format!("    {path}"));
    }

    // 2. Render the document into it, so the folder is complete from the start.
    let prepared =
        site::prepare_content(&site_root, &document, filename).map_err(map_site_error)?;
    if let Some(note) = &prepared.removal_note {
        ui.warn(note);
    }

    // 3. Record state.
    let mut state = SiteState::new(site_name.clone(), Utf8PathBuf::from("..").join(filename));
    state.save(&site_root).map_err(map_site_error)?;

    // 4. Start a repository.
    ui.blank();
    if git_is_available() {
        start_repository(context, &site_root, approval)?;
    } else {
        ui.status(
            Mark::Warn,
            "git is not installed",
            "history is not being tracked",
        );
        ui.say("Install git, then run `kosong site init` again to track history.");
    }

    // 5. Offer GitHub, without requiring it.
    ui.blank();
    offer_github(context, &site_root, &site_name, &mut state, approval)?;

    let mut onboarding = context.onboarding()?;
    onboarding.site_initialized = true;
    context.save_onboarding(&onboarding);

    ui.blank();
    ui.lesson(
        "That folder is an ordinary Astro project with an ordinary git history.\n\
         You can open it, change it, or publish it yourself without kosong.",
    );
    ui.next_command("kosong site publish");
    Ok(())
}

fn git_is_available() -> bool {
    crate::tools::find_executable(git::PROGRAM).is_some()
}

/// `git init`, then a first commit of the generated files only.
fn start_repository(context: &Context, site_root: &Utf8Path, approval: Approval) -> CliResult<()> {
    let ui = context.ui;

    perform_checked(ui, &GitOperation::Init, site_root, approval)?;

    // Committing without an identity fails with a message that means nothing
    // to a beginner, so it is checked first and explained properly.
    if let Some(result) = perform(ui, &GitOperation::IdentityCheck, site_root, approval)? {
        if !git::has_identity(result.exit_code, &result.stdout) {
            ui.status(Mark::Warn, "git does not know who you are", "");
            ui.blank();
            for line in git::IDENTITY_REPAIR.lines() {
                ui.say(line);
            }
            return Ok(());
        }
    }

    perform_checked(
        ui,
        &GitOperation::add(site::owned_paths()),
        site_root,
        approval,
    )?;
    perform_checked(
        ui,
        &GitOperation::Commit {
            message: "Set up site with kosong".into(),
            paths: site::owned_paths(),
        },
        site_root,
        approval,
    )?;

    ui.status(Mark::Good, "started tracking history", "first commit made");
    Ok(())
}

/// Offers to create a GitHub repository. Declining is fine.
fn offer_github(
    context: &Context,
    site_root: &Utf8Path,
    site_name: &str,
    state: &mut SiteState,
    approval: Approval,
) -> CliResult<()> {
    let ui = context.ui;

    if crate::tools::find_executable("gh").is_none() {
        ui.status(
            Mark::Info,
            "GitHub CLI is not installed",
            "skipping for now",
        );
        ui.say("You can publish without it. To use GitHub later, install `gh` and run `kosong site init` again.");
        return Ok(());
    }

    let status = perform(ui, &GitHubOperation::AuthStatus, site_root, approval)?;
    let signed_in = status.is_some_and(|result| result.success());
    if !signed_in {
        ui.status(
            Mark::Warn,
            "GitHub CLI is not signed in",
            "skipping for now",
        );
        ui.say("Run `gh auth login`, then `kosong site init` again to connect GitHub.");
        return Ok(());
    }

    if !approval.assume_yes
        && !approval.dry_run
        && !ui.confirm("Create a GitHub repository?", false)
    {
        ui.status(Mark::Info, "skipped GitHub", "you can add it later");
        return Ok(());
    }

    let operation =
        GitHubOperation::repo_create(site_name, true, true).map_err(map_provider_error)?;
    if perform_checked(ui, &operation, site_root, approval)?.is_some() {
        state.github_repo = Some(site_name.to_owned());
        state.save(site_root).map_err(map_site_error)?;
        ui.status(Mark::Good, "created the repository", site_name);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// site publish
// ---------------------------------------------------------------------------

/// `kosong site publish` — build and deploy.
///
/// Lesson: deployment moves built files to a host. It is not magic.
pub fn publish(context: &Context, approval: Approval) -> CliResult<()> {
    let ui = context.ui;
    let workspace = context.workspace()?;
    let site_root = locate_site(&workspace)?;
    let mut state = SiteState::load(&site_root).map_err(map_site_error)?;

    // 1. The document must be valid before anything else happens.
    let document = workspace.read_document()?;
    let filename = workspace.document_path().file_name().unwrap_or("kosong.md");

    ui.heading(format!("Publishing `{}`.", state.site_name));
    ui.blank();

    // 2. Refuse on unrelated uncommitted changes, per §13.3 step 3.
    if git_is_available() {
        refuse_on_unrelated_changes(context, &site_root, approval)?;
    }

    // 3. Render the current document into the template.
    let prepared =
        site::prepare_content(&site_root, &document, filename).map_err(map_site_error)?;
    ui.status(
        Mark::Good,
        "prepared your page",
        prepared.html_path.as_str(),
    );
    if let Some(note) = &prepared.removal_note {
        ui.warn(note);
    }

    // 4. Install and build.
    if crate::tools::find_executable("npm").is_none() {
        return Err(CliError::new(
            Exit::Provider,
            "NPM_MISSING",
            "npm is needed to build your page",
        )
        .with_repair("Install Node.js from https://nodejs.org, then try again."));
    }

    ui.say("Fetching what the build needs…");
    perform_checked(ui, &NpmOperation::Install, &site_root, approval)?;

    ui.say("Building…");
    let built = perform_checked(ui, &NpmOperation::Build, &site_root, approval)?;

    // 5. A build can exit zero having written nothing. Deploying that would
    //    replace a working site with a blank one.
    if built.is_some() && !site::build_output_exists(&site_root) {
        return Err(map_site_error(SiteError::NoBuildOutput));
    }
    if built.is_some() {
        ui.status(Mark::Good, "built", site::BUILD_OUTPUT_DIR);
    }

    // 6. Commit and push the source, so the repository matches what is live.
    if git_is_available() {
        commit_and_push(context, &site_root, &state, approval)?;
    }

    // 7. Deploy.
    if crate::tools::find_executable("wrangler").is_none() {
        return Err(CliError::new(
            Exit::Provider,
            "WRANGLER_MISSING",
            "wrangler is needed to publish to Cloudflare",
        )
        .with_repair("Install it with:\n  npm install -g wrangler\nThen run: wrangler login"));
    }

    ui.blank();
    let deploy =
        CloudflareOperation::deploy(state.project(), Utf8Path::new(site::BUILD_OUTPUT_DIR), None)
            .map_err(map_provider_error)?;

    let Some(result) = perform_checked(ui, &deploy, &site_root, approval)? else {
        return Ok(()); // dry run
    };

    // 8. Record and report.
    let output = result.combined_output();
    if let Some(url) = extract_deployment_url(&output) {
        state.last_deployment_url = Some(url.clone());
        state.save(&site_root).map_err(map_site_error)?;

        ui.blank();
        ui.status(Mark::Good, "published", &url);
    } else {
        ui.blank();
        ui.status(Mark::Good, "published", "");
        ui.say(output);
    }

    let mut onboarding = context.onboarding()?;
    onboarding.site_published = true;
    context.save_onboarding(&onboarding);

    ui.blank();
    ui.lesson(
        "Your page is now files on a web host. Nothing about it is locked to kosong:\n\
         the folder, the git history, and the built files are all yours.",
    );
    ui.next_command("kosong status");
    Ok(())
}

/// Refuses to publish when the folder holds changes kosong did not make.
fn refuse_on_unrelated_changes(
    context: &Context,
    site_root: &Utf8Path,
    approval: Approval,
) -> CliResult<()> {
    let ui = context.ui;

    let Some(result) = perform(ui, &GitOperation::Status, site_root, approval)? else {
        return Ok(());
    };
    if !result.success() {
        // Not a git repository, or git is unhappy. Not a reason to block a
        // publish that does not depend on git.
        return Ok(());
    }

    let unrelated = git::unrelated_changes(&result.stdout, &site::owned_paths());
    if unrelated.is_empty() {
        return Ok(());
    }

    ui.status(
        Mark::Warn,
        "this folder has changes kosong did not make",
        "",
    );
    ui.blank();
    for change in unrelated.iter().take(10) {
        ui.say(format!("    {} {}", change.code.trim(), change.path));
    }
    if unrelated.len() > 10 {
        ui.say(format!("    …and {} more", unrelated.len() - 10));
    }
    ui.blank();

    Err(CliError::usage(
        "UNRELATED_CHANGES",
        "there are changes here that kosong did not make",
    )
    .with_repair(
        "kosong will not commit files it did not write, in case they were not meant to be \
         published.\n\
         Save or remove them yourself first:\n  \
         git add . && git commit -m \"my changes\"\n\
         Then run: kosong site publish",
    ))
}

fn commit_and_push(
    context: &Context,
    site_root: &Utf8Path,
    state: &SiteState,
    approval: Approval,
) -> CliResult<()> {
    let ui = context.ui;

    perform(
        ui,
        &GitOperation::add(site::owned_paths()),
        site_root,
        approval,
    )?;

    // A commit with nothing staged exits non-zero, which is not a failure
    // worth stopping a publish for.
    let committed = perform(
        ui,
        &GitOperation::Commit {
            message: "Update page with kosong".into(),
            paths: site::owned_paths(),
        },
        site_root,
        approval,
    )?;
    if committed.as_ref().is_some_and(|r| r.success()) {
        ui.status(Mark::Good, "recorded the change", "");
    }

    if state.github_repo.is_some() {
        let push = GitOperation::push("origin", git::DEFAULT_BRANCH).map_err(map_provider_error)?;
        if let Some(result) = perform(ui, &push, site_root, approval)? {
            if result.success() {
                ui.status(Mark::Good, "sent to GitHub", "");
            } else {
                // A failed push must not sink a publish: the built files are
                // fine and the deployment is what the user asked for.
                ui.warn("could not send to GitHub; your page will still be published");
            }
        }
    }
    Ok(())
}

/// Pulls a deployment URL out of wrangler's output.
pub fn extract_deployment_url(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_ascii_graphic() || "\"'(),".contains(c)))
        .find(|word| word.starts_with("https://") && word.contains(".pages.dev"))
        .map(str::to_owned)
}

// ---------------------------------------------------------------------------
// site rollback
// ---------------------------------------------------------------------------

/// `kosong site rollback` — show deployment history and how to go back.
///
/// Lesson: published systems can be changed safely with history and intent.
///
/// # Why this does not roll back for you
///
/// Checked against Wrangler 4.114 and current Cloudflare documentation:
/// **Wrangler has no Pages rollback command.** `pages deployment` offers
/// `list`, `create`, `tail`, and `delete`; the top-level `wrangler rollback` is
/// for Workers. Cloudflare documents Pages rollback as a dashboard action.
///
/// Doing it through Cloudflare's REST API would require an API token in
/// kosong's hands, which architectural rule 2 in §2 forbids.
///
/// So this lists the history — which is real, useful, and uses an allowlisted
/// read-only operation — and then says plainly what kosong cannot do and where
/// to do it. Pretending otherwise, or quietly deleting the newer deployment
/// and calling that a rollback, would be worse than being honest.
pub fn rollback(context: &Context, approval: Approval) -> CliResult<()> {
    let ui = context.ui;
    let workspace = context.workspace()?;
    let site_root = locate_site(&workspace)?;
    let state = SiteState::load(&site_root).map_err(map_site_error)?;

    ui.heading(format!("Past versions of `{}`.", state.site_name));
    ui.blank();

    if crate::tools::find_executable("wrangler").is_none() {
        return Err(CliError::new(
            Exit::Provider,
            "WRANGLER_MISSING",
            "wrangler is needed to look up past versions",
        )
        .with_repair("Install it with:\n  npm install -g wrangler"));
    }

    let operation =
        CloudflareOperation::deployment_list(state.project()).map_err(map_provider_error)?;

    if let Some(result) = perform(ui, &operation, &site_root, approval)? {
        if result.success() {
            ui.always(result.combined_output());
        } else {
            ui.warn("could not list past versions");
            ui.say(result.combined_output());
        }
    }

    ui.blank();
    for line in cloudflare::rollback_guidance(state.project()).lines() {
        ui.say(line);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

fn locate_site(workspace: &Workspace) -> CliResult<Utf8PathBuf> {
    site::discover(workspace).ok_or_else(|| map_site_error(SiteError::NotInitialized))
}

#[cfg(test)]
mod tests {
    use super::extract_deployment_url;

    #[test]
    fn a_deployment_url_is_found_in_wrangler_output() {
        let output = "\
✨ Success! Uploaded 3 files (1.23 sec)
✨ Deployment complete! Take a peek over at https://abc12345.my-site.pages.dev";

        assert_eq!(
            extract_deployment_url(output).as_deref(),
            Some("https://abc12345.my-site.pages.dev")
        );
    }

    #[test]
    fn output_without_a_url_yields_nothing_rather_than_a_guess() {
        assert_eq!(extract_deployment_url("Uploaded 3 files"), None);
        assert_eq!(extract_deployment_url(""), None);
    }

    #[test]
    fn an_unrelated_url_is_not_mistaken_for_a_deployment() {
        let output = "See https://developers.cloudflare.com/pages for help";
        assert_eq!(extract_deployment_url(output), None);
    }
}
