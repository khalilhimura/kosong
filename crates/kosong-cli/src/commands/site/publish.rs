// ---------------------------------------------------------------------------
// site publish
// ---------------------------------------------------------------------------

use super::*;
use kosong_core::providers::cloudflare::{self, CloudflareOperation, ProjectCreateOutcome};
use kosong_core::providers::git::{self, GitOperation};
use kosong_core::providers::github::GitHubOperation;
use kosong_core::providers::npm::NpmOperation;
use kosong_core::site::{self, SiteState};
use kosong_core::providers::AuthState;

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

    // 2. Everything this needs, before anything is fetched, built, or sent.
    preflight(ui, &state, &site_root, approval)?;

    // 3. Refuse on unrelated uncommitted changes, per §13.3 step 3.
    if git_is_available() {
        refuse_on_unrelated_changes(context, &site_root, approval)?;
    }

    // 4. Render the current document into the template.
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

    // 5. Install and build. npm's presence was settled by the preflight.
    ui.say("Fetching what the build needs…");
    perform_checked(ui, &NpmOperation::Install, &site_root, approval)?;

    ui.say("Building…");
    let built = perform_checked(ui, &NpmOperation::Build, &site_root, approval)?;

    // 6. A build can exit zero having written nothing. Deploying that would
    //    replace a working site with a blank one.
    if built.is_some() && !site::build_output_exists(&site_root) {
        return Err(map_site_error(SiteError::NoBuildOutput));
    }
    if built.is_some() {
        ui.status(Mark::Good, "built", site::BUILD_OUTPUT_DIR);
    }

    // 7. Commit and push the source, so the repository matches what is live.
    if git_is_available() {
        commit_and_push(context, &site_root, &state, approval)?;
    }

    // 8. Deploy. wrangler was settled by the preflight.
    ui.blank();
    ensure_project_exists(ui, &state, &site_root, approval)?;

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
        "A place had to exist before your files could be put in it — that is what the first \
         step made.\n\
         Your page is now files on a web host. Nothing about it is locked to kosong:\n\
         the folder, the git history, and the built files are all yours.",
    );
    ui.next_command("kosong status");
    Ok(())
}

// ---------------------------------------------------------------------------
// The publish preflight
// ---------------------------------------------------------------------------

/// A reason `publish` cannot start.
struct Blocker {
    code: &'static str,
    problem: String,
    repair: String,
}

/// Checks everything `publish` depends on, before it fetches, builds, or sends.
fn preflight(ui: Ui, state: &SiteState, site_root: &Utf8Path, approval: Approval) -> CliResult<()> {
    let mut blockers = Vec::new();

    if crate::tools::locate("npm", Some(site_root)).is_none() {
        blockers.push(Blocker {
            code: "NPM_MISSING",
            problem: "npm is needed to build your page".into(),
            repair: "Install Node.js from https://nodejs.org, then try again.".into(),
        });
    }

    if crate::tools::locate(cloudflare::PROGRAM, Some(site_root)).is_none() {
        blockers.push(wrangler_is_missing(site_root, "publish to Cloudflare"));
    }

    report(ui, blockers)?;

    // Sign-in, only once wrangler is known to be there — asking a tool that is
    // absent produces a spawn failure, not an answer.
    check_cloudflare_sign_in(ui, state, site_root, approval)?;

    if state.github_repo.is_some() {
        warn_about_github(ui, site_root, approval);
    }

    Ok(())
}

/// Turns collected blockers into one failure, naming all of them.
fn report(ui: Ui, mut blockers: Vec<Blocker>) -> CliResult<()> {
    match blockers.len() {
        0 => Ok(()),
        1 => {
            let blocker = blockers.remove(0);
            Err(CliError::provider(blocker.code, blocker.problem).with_repair(blocker.repair))
        }
        _ => {
            for blocker in &blockers {
                ui.status(Mark::Bad, &blocker.problem, "");
                ui.blank();
                for line in blocker.repair.lines() {
                    ui.say(format!("    {line}"));
                }
                ui.blank();
            }
            Err(CliError::provider(
                "PREFLIGHT_FAILED",
                format!(
                    "{} things need setting up before you can publish",
                    blockers.len()
                ),
            )
            .with_repair("Each ✖ above says what to do. Then run: kosong site publish"))
        }
    }
}

/// What to tell someone whose wrangler kosong cannot find.
fn wrangler_is_missing(site_root: &Utf8Path, purpose: &str) -> Blocker {
    let install = format!(
        "Install it into your site folder, the way Cloudflare recommends:\n  \
         cd {site_root}\n  npm i -D wrangler@latest\n  npx wrangler login"
    );

    match crate::tools::unreachable_install(cloudflare::PROGRAM) {
        Some(path) => {
            let folder = path
                .parent()
                .map_or_else(|| path.clone(), Utf8Path::to_owned);
            Blocker {
                code: "WRANGLER_UNREACHABLE",
                problem: "wrangler is installed, but your shell cannot find it".into(),
                repair: format!(
                    "kosong found it here:\n  {path}\n\
                     \n\
                     But this folder is not in your PATH, so nothing can run it by name:\n  \
                     {folder}\n\
                     \n\
                     That is why `wrangler login` says \"command not found\" even though\n\
                     `npm install -g wrangler` said it worked. A folder like that can also\n\
                     belong to an application, and an application update can quietly remove\n\
                     everything installed into it.\n\
                     \n\
                     {install}"
                ),
            }
        }
        None => Blocker {
            code: "WRANGLER_MISSING",
            problem: format!("wrangler is needed to {purpose}"),
            repair: format!(
                "kosong looked in:\n  {local}\n  every folder in your PATH\n\n{install}",
                local = site_root.join(crate::tools::LOCAL_BIN),
            ),
        },
    }
}

/// Stops a publish by someone who is not signed in to Cloudflare.
fn check_cloudflare_sign_in(
    ui: Ui,
    state: &SiteState,
    site_root: &Utf8Path,
    approval: Approval,
) -> CliResult<()> {
    if state.last_deployment_url.is_some() {
        return Ok(());
    }

    let Some(result) = perform(ui, &CloudflareOperation::WhoAmI, site_root, approval)? else {
        return Ok(());
    };

    match cloudflare::interpret_whoami(result.exit_code, &result.combined_output()) {
        AuthState::SignedIn => Ok(()),
        AuthState::SignedOut => Err(CliError::provider(
            "WRANGLER_SIGNED_OUT",
            "wrangler is not signed in to Cloudflare",
        )
        .with_repair(
            "Nothing has been built or published yet.\n\
             \n\
             Sign in, from your site folder:\n  \
             npx wrangler login\n\
             \n\
             That opens your browser. Then run: kosong site publish",
        )),
        AuthState::Unknown => {
            ui.status(
                Mark::Warn,
                "could not tell whether wrangler is signed in",
                "carrying on",
            );
            ui.say("If the publish fails, wrangler's own message will say why.");
            Ok(())
        }
    }
}

/// Warns about GitHub. Never blocks.
fn warn_about_github(ui: Ui, site_root: &Utf8Path, approval: Approval) {
    let warn = |detail: &str| {
        ui.status(Mark::Warn, detail, "your page will still be published");
        ui.blank();
        ui.say("    Only the copy of your history on GitHub is affected.");
        ui.say("    Run this once, then publishing will send it up too:");
        ui.say("      gh auth login");
        ui.say("      gh auth setup-git");
        ui.blank();
    };

    if crate::tools::locate("gh", Some(site_root)).is_none() {
        warn("GitHub CLI is not installed");
        return;
    }

    let signed_in = perform(ui, &GitHubOperation::AuthStatus, site_root, approval)
        .ok()
        .flatten()
        .is_some_and(|result| result.success());

    if !signed_in {
        warn("GitHub CLI is not signed in");
    }
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

    let staged = super::stageable(ui, site_root, site::owned_paths(), approval)?;

    perform(ui, &GitOperation::add(staged.clone()), site_root, approval)?;

    let committed = perform(
        ui,
        &GitOperation::Commit {
            message: "Update page with kosong".into(),
            paths: staged,
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
                ui.warn("could not send to GitHub; your page will still be published");

                let reason = result.combined_output();
                ui.blank();
                for line in reason.lines() {
                    ui.say(format!("    {line}"));
                }

                if git::needs_credentials(&reason) {
                    ui.blank();
                    for line in git::PUSH_REPAIR.lines() {
                        ui.say(line);
                    }
                }
            }
        }
    }
    Ok(())
}

fn git_is_available() -> bool {
    super::git_is_available()
}

/// Makes sure the Cloudflare Pages project exists before anything is deployed
/// into it.
fn ensure_project_exists(
    ui: Ui,
    state: &SiteState,
    site_root: &Utf8Path,
    approval: Approval,
) -> CliResult<()> {
    let create = CloudflareOperation::project_create(state.project(), Some(git::DEFAULT_BRANCH))
        .map_err(map_provider_error)?;

    let Some(listed) = perform_checked(
        ui,
        &CloudflareOperation::PagesProjectList,
        site_root,
        approval,
    )?
    else {
        return Err(CliError::internal(
            "PROJECT_LIST_SKIPPED",
            "the check for an existing Cloudflare project did not run",
        )
        .with_repair(
            "This is a bug in kosong, not something you did. Nothing was published.\n\
             Please report it, and mention that `site publish` skipped the project check.",
        ));
    };

    if project_exists(&listed.stdout, state.project()) {
        return Ok(());
    }

    let Some(result) = perform(ui, &create, site_root, approval)? else {
        return Ok(());
    };

    match cloudflare::interpret_project_create(result.exit_code, &result.combined_output()) {
        ProjectCreateOutcome::Created => {
            ui.status(Mark::Good, "made a place for your page", state.project());
            Ok(())
        }
        ProjectCreateOutcome::NameTaken => Err(CliError::provider(
            "PROJECT_NAME_TAKEN",
            format!("`{}` is already taken on Cloudflare", state.project()),
        )
        .with_repair(format!(
            "Cloudflare project names are unique to your account, and `{}` is in use.\n\
             Pick another name — edit `cloudflare_project` in `.kosong/site.toml`,\n\
             or start again with: kosong site init <another-name>",
            state.project()
        ))),
        ProjectCreateOutcome::Failed => Err(CliError::provider(
            "TOOL_FAILED",
            format!("`{}` failed", result.executable),
        )
        .with_repair(result.combined_output())),
    }
}

/// Pulls a deployment URL out of wrangler's output.
fn extract_deployment_url(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_ascii_graphic() || "\"'(),".contains(c)))
        .find(|word| word.starts_with("https://") && word.contains(".pages.dev"))
        .map(str::to_owned)
}

/// Whether `name` is one of the projects in `wrangler pages project list`.
fn project_exists(output: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    output
        .lines()
        .filter_map(|line| line.split('│').nth(1))
        .any(|cell| cell.trim() == name)
}
