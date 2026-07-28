//! `kosong site init`, `publish`, and `rollback`.
//!
//! Every mutating step follows §12.4: validate, build a typed plan, disclose
//! executable and argv and working directory and files and remote effect, stop
//! on `--dry-run`, and require confirmation unless `--yes`.

use super::Context;
use super::provider::{map_process_error, map_provider_error};
use crate::exit::{CliError, CliResult};
use crate::ui::{Mark, Ui};
use camino::{Utf8Path, Utf8PathBuf};
use kosong_core::process::ProcessResult;
use kosong_core::providers::Operation;
use kosong_core::providers::cloudflare::{self, CloudflareOperation, ProjectCreateOutcome};
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
        Err(
            CliError::provider("TOOL_FAILED", format!("`{}` failed", result.executable))
                .with_repair(result.combined_output()),
        )
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

    // `package-lock.json` is owned but does not exist yet: npm writes it at
    // publish step 4, long after this runs.
    let staged = present(site_root, site::owned_paths());

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

/// The owned paths that are actually present in `site_root`.
///
/// [`site::owned_paths`] names what belongs to the site, which is not the same
/// as what is on disk right now — `package-lock.json` is ours from the moment
/// npm writes it, and absent for the whole of `site init`. Both git operations
/// that receive the list refuse a path they cannot find, in different ways and
/// with different exit codes:
///
/// - `git add -- missing.txt` exits **128** with
///   `fatal: pathspec 'missing.txt' did not match any files`. There is no
///   `--ignore-unmatch` to soften it; that is a `git rm` option.
/// - `git commit --only -- missing.txt` exits **1** with
///   `error: pathspec ... did not match any file(s) known to git`, and its bar
///   is higher still: a file that exists but has never been staged is just as
///   unknown. Filtering only the `add` list would therefore trade one broken
///   command for another, because `--only` would then be handed a lockfile
///   `add` had just skipped.
///
/// Both were run and observed rather than inferred from the documentation.
///
/// The cost is that a *tracked* file the user deleted is no longer staged as a
/// deletion, since it fails the same existence test. Recovering that would mean
/// asking git what it tracks — a new operation, and §12.2 keeps that list
/// short — to serve a state that stops the publish at `npm install` or the
/// build well before any commit is reached.
///
/// The result is never empty in practice: `prepare_content` writes
/// `src/content` before either caller runs, and `atomic_write` creates the
/// directories on the way. That matters, because `git commit --only` with no
/// paths at all is itself a fatal error rather than a no-op.
fn present(site_root: &Utf8Path, paths: Vec<Utf8PathBuf>) -> Vec<Utf8PathBuf> {
    paths
        .into_iter()
        .filter(|path| site_root.join(path).exists())
        .collect()
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
        return Err(
            CliError::provider("NPM_MISSING", "npm is needed to build your page")
                .with_repair("Install Node.js from https://nodejs.org, then try again."),
        );
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
        return Err(CliError::provider(
            "WRANGLER_MISSING",
            "wrangler is needed to publish to Cloudflare",
        )
        .with_repair("Install it with:\n  npm install -g wrangler\nThen run: wrangler login"));
    }

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

    // By now `npm install` has run, so the lockfile is here and is committed
    // along with everything else. That is what adopts the stray lockfile in a
    // site created before it was owned: no migration, just the next publish.
    let staged = present(site_root, site::owned_paths());

    perform(ui, &GitOperation::add(staged.clone()), site_root, approval)?;

    // A commit with nothing staged exits non-zero, which is not a failure
    // worth stopping a publish for.
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
                // A failed push must not sink a publish: the built files are
                // fine and the deployment is what the user asked for.
                ui.warn("could not send to GitHub; your page will still be published");
            }
        }
    }
    Ok(())
}

/// Makes sure the Cloudflare Pages project exists before anything is deployed
/// into it.
///
/// `wrangler pages deploy --project-name X` fails outright when `X` does not
/// exist, and tells the user to run a command kosong does not offer. Creating
/// it here is what makes a first publish work at all.
///
/// The create is disclosed and confirmed on its own rather than folded into the
/// deploy's approval. Of the two Cloudflare steps, a first publish asks about
/// both and every publish after asks only about the deploy — which is the
/// intent, because the second time there is no project left to make. (That
/// count is the Cloudflare pair alone. A site with a GitHub remote is asked
/// about the push too, earlier, in `commit_and_push`.) Folding the create into
/// the deploy would run a mutating remote operation with no disclosure of its
/// own, which §12.4 does not bend on.
fn ensure_project_exists(
    ui: Ui,
    state: &SiteState,
    site_root: &Utf8Path,
    approval: Approval,
) -> CliResult<()> {
    // `main` is always stated. Left unset, wrangler asks which branch is
    // production, and kosong runs it with no terminal to answer from. It
    // matches the branch `site init` creates.
    //
    // Built here, before the list, rather than at its point of use below,
    // because constructing it is what validates the name against Cloudflare's
    // rule — and `project_exists` documents that it is handed an
    // already-validated name. Nothing else in `publish` has validated
    // `state.project()` by this point: the name comes from `.kosong/site.toml`,
    // which a user can edit, and the deploy that would have validated it is
    // still ahead. Validating first also means an unusable name is refused
    // before kosong says anything to Cloudflare. That saves the list and the
    // create, and nothing more: `npm install` has already fetched from the
    // network by now, and a site with a GitHub remote has already pushed.
    let create = CloudflareOperation::project_create(state.project(), Some(git::DEFAULT_BRANCH))
        .map_err(map_provider_error)?;

    // A failed list is not an absent project. An auth failure, a network error,
    // and a rate limit all look like "the name is not in the output" if only
    // the text is consulted — and the create that followed would fail for the
    // same reason while blaming the wrong thing. `perform_checked` stops here
    // with wrangler's own error instead.
    //
    // This is also the one way a `--dry-run` can now fail, which it could not
    // before: the list is read-only, so it runs even under a dry run, and
    // someone signed out or offline gets wrangler's error where they used to
    // get a plan. That cost is accepted deliberately. A dry run that cannot
    // read the project list cannot know whether a create belongs in the plan,
    // and printing the plan without it would be a guess presented as a fact —
    // the same wrong answer as treating a failed list as an absent project,
    // just delivered more politely.
    let Some(listed) = perform_checked(
        ui,
        &CloudflareOperation::PagesProjectList,
        site_root,
        approval,
    )?
    else {
        // Unreachable: `perform` returns `None` only where `--dry-run` stops a
        // *mutating* operation, and the list is read-only. Said out loud rather
        // than shrugged at, because the failure mode is silent: were
        // `PagesProjectList::mutating()` ever flipped, an `Ok(())` here would
        // skip the existence check and let the publish deploy blind, which is
        // the one outcome this function exists to prevent.
        return Err(CliError::internal(
            "PROJECT_LIST_SKIPPED",
            "the check for an existing Cloudflare project did not run",
        )
        .with_repair(
            "This is a bug in kosong, not something you did. Nothing was published.\n\
             Please report it, and mention that `site publish` skipped the project check.",
        ));
    };

    // `stdout`, not `combined_output()`. The table is what wrangler prints on
    // success, and `combined_output()` is documented for showing a failure: it
    // puts stderr first, so wrangler's banner and any warning would be fed to
    // the table parser as though they were rows. Reading the stream the answer
    // actually arrives on is the honest version of this check.
    //
    // It buys no protection from redaction, which applies to stdout and stderr
    // alike as they are read. A project whose name happens to contain a
    // credential prefix — `redact_prefixes` matches those anywhere in a line,
    // not only at a word boundary — is blanked out of the table either way, so
    // kosong would try to create a project that already exists and then tell
    // the user to rename a name that is already theirs. Narrow, and a fix
    // belongs in the redaction rather than here.
    if project_exists(&listed.stdout, state.project()) {
        return Ok(());
    }

    // Returning `Ok(())` on a dry run returns to `publish`, which carries on to
    // disclose the deploy. Returning from `publish` itself here would stop the
    // plan after naming only half of it.
    let Some(result) = perform(ui, &create, site_root, approval)? else {
        return Ok(());
    };

    match cloudflare::interpret_project_create(result.exit_code, &result.combined_output()) {
        ProjectCreateOutcome::Created => {
            ui.status(Mark::Good, "made a place for your page", state.project());
            Ok(())
        }
        // Pages project names are unique per account. Proceeding to deploy
        // would publish into a project the user did not mean to touch.
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
        // Word for word what `perform_checked` would have produced. The create
        // cannot go through `perform_checked`, because a taken name has to be
        // separated out above before the generic failure is reached — so the
        // generic failure has to be rebuilt here rather than inherited.
        ProjectCreateOutcome::Failed => Err(CliError::provider(
            "TOOL_FAILED",
            format!("`{}` failed", result.executable),
        )
        .with_repair(result.combined_output())),
    }
}

/// Pulls a deployment URL out of wrangler's output.
pub fn extract_deployment_url(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_ascii_graphic() || "\"'(),".contains(c)))
        .find(|word| word.starts_with("https://") && word.contains(".pages.dev"))
        .map(str::to_owned)
}

/// Whether `name` is one of the projects in `wrangler pages project list`.
///
/// wrangler prints a padded box-drawing table. Splitting on the cell separator
/// and comparing the first cell exactly is the whole point: a substring test
/// would find `kosong` inside `kosong-smoke-30270267641` and skip a create
/// that was needed.
///
/// Rule lines (`├─┼─┤`) carry no separator and drop out. `name` is expected
/// to already be a validated Cloudflare project name (lowercase, digits,
/// hyphens — see `validate_project_name` in `kosong_core::providers::cloudflare`),
/// which is why the header row's `Project Name` cell is not a hazard in
/// practice: no caller can pass it.
pub fn project_exists(output: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    output
        .lines()
        .filter_map(|line| line.split('│').nth(1))
        .any(|cell| cell.trim() == name)
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
        return Err(CliError::provider(
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
    use super::{extract_deployment_url, project_exists};

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

    /// The real shape of `wrangler pages project list`, captured from Wrangler
    /// 4.114.0. Names are invented; the account's own are not committed.
    const PROJECT_TABLE: &str = "\
 ⛅️ wrangler 4.114.0
────────────────────
┌─────────────────┬──────────────────────────────┬──────────────┬───────────────┐
│ Project Name    │ Project Domains              │ Git Provider │ Last Modified │
├─────────────────┼──────────────────────────────┼──────────────┼───────────────┤
│ kosong          │ kosong-nwb.pages.dev         │ No           │ 6 hours ago   │
├─────────────────┼──────────────────────────────┼──────────────┼───────────────┤
│ my-first-site   │ my-first-site.pages.dev      │ No           │ 2 weeks ago   │
└─────────────────┴──────────────────────────────┴──────────────┴───────────────┘";

    #[test]
    fn a_project_is_found_by_its_whole_name_not_a_substring() {
        assert!(project_exists(PROJECT_TABLE, "kosong"));
        assert!(project_exists(PROJECT_TABLE, "my-first-site"));

        // The bug this guards: `output.contains("kosong")` is true here, so a
        // smoke run named `kosong-smoke-30270267641` would be judged to exist
        // and its create skipped, and the deploy would then fail with the very
        // error this whole change removes.
        assert!(!project_exists(PROJECT_TABLE, "kosong-smoke-30270267641"));
        assert!(!project_exists(PROJECT_TABLE, "koson"));
        assert!(!project_exists(PROJECT_TABLE, "my-first-site-2"));
    }

    #[test]
    fn empty_name_or_table_finds_nothing() {
        // The header row ("Project Name", "Project Domains") is never a match
        // because every caller passes an already-validated Cloudflare project
        // name (lowercase, digits, hyphens), which cannot equal the literal
        // string "Project Name".
        //
        // Rule lines (├─┼─┤ etc.) have no `│` cell separator. The filter_map
        // in project_exists splits on `│` and takes nth(1), so rule lines return
        // None and are dropped before any comparison happens.
        assert!(!project_exists(PROJECT_TABLE, ""));
        assert!(!project_exists("", "kosong"));
    }
}
