// ---------------------------------------------------------------------------
// site init
// ---------------------------------------------------------------------------

use super::*;
use kosong_core::providers::cloudflare;
use kosong_core::providers::git::{self, GitOperation};
use kosong_core::providers::github::GitHubOperation;
use kosong_core::site::{self, SiteState};
use kosong_core::SiteError;

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

    // Said here rather than left for the publish to discover. Cloudflare's own
    // tool is the one thing publishing cannot do without, and learning that by
    // failing a publish means learning it after a build has been paid for.
    if crate::tools::locate(cloudflare::PROGRAM, Some(&site_root)).is_none() {
        ui.blank();
        ui.say("Publishing needs Cloudflare's own tool. Set it up once, in your site folder:");
        ui.say(format!("  cd {site_root}"));
        ui.say("  npm i -D wrangler@latest");
        ui.say("  npx wrangler login");
    }

    ui.next_command("kosong site publish");
    Ok(())
}

fn git_is_available() -> bool {
    super::git_is_available()
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

    let staged = super::stageable(ui, site_root, site::owned_paths(), approval)?;

    perform_checked(ui, &GitOperation::add(staged.clone()), site_root, approval)?;
    perform_checked(
        ui,
        &GitOperation::Commit {
            message: "Set up site with kosong".into(),
            paths: staged,
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
