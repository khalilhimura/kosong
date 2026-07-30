//! `kosong doctor` — check that everything needed is in place.
//!
//! Lesson: prerequisites can be checked, not guessed.
//!
//! Every failing check must carry a repair action. A diagnostic that reports a
//! problem without a next step is the exact experience `kosong` exists to
//! replace, and the Phase A exit gate makes it a release condition.
//!
//! Tool checks here are presence-only, by reading `PATH`. Checking whether
//! `gh` or `wrangler` is *signed in* means running them, which arrives with
//! the process adapters.

use super::Context;
use crate::exit::{CliError, CliResult};
use crate::tools::{self, TOOLS};
use crate::ui::Mark;
use kosong_core::config;
use kosong_core::providers::Operation;
use kosong_core::providers::cloudflare::{self, CloudflareOperation};
use kosong_core::providers::github::{self, GitHubOperation};
use serde::Serialize;

const SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Health {
    /// Working.
    Ok,
    /// Not working, but nothing local is blocked.
    Warn,
    /// Local work is blocked.
    Fail,
}

impl Health {
    fn mark(self) -> Mark {
        match self {
            Self::Ok => Mark::Good,
            Self::Warn => Mark::Warn,
            Self::Fail => Mark::Bad,
        }
    }
}

#[derive(Debug, Serialize)]
struct Check {
    name: String,
    status: Health,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    repair: Option<String>,
}

impl Check {
    fn ok(name: &str, detail: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            status: Health::Ok,
            detail: detail.into(),
            repair: None,
        }
    }

    fn warn(name: &str, detail: impl Into<String>, repair: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            status: Health::Warn,
            detail: detail.into(),
            repair: Some(repair.into()),
        }
    }

    fn fail(name: &str, detail: impl Into<String>, repair: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            status: Health::Fail,
            detail: detail.into(),
            repair: Some(repair.into()),
        }
    }
}

#[derive(Debug, Serialize)]
struct Report {
    schema: u32,
    okf_version: &'static str,
    /// True when nothing is in a failing state.
    healthy: bool,
    checks: Vec<Check>,
}

pub fn run(context: &Context, json: bool) -> CliResult<()> {
    let ui = context.ui;

    let mut checks = Vec::new();
    checks.push(check_config_dir());
    checks.push(check_editor(context));
    checks.extend(check_document(context));
    let site_root = site_root(context);
    checks.extend(check_tools(site_root.as_deref()));
    checks.extend(check_provider_auth(context, site_root.as_deref()));

    let healthy = checks.iter().all(|c| c.status != Health::Fail);
    let report = Report {
        schema: SCHEMA,
        okf_version: kosong_core::okf::OKF_VERSION,
        healthy,
        checks,
    };

    if json {
        let text = serde_json::to_string_pretty(&report).map_err(|e| {
            CliError::internal("JSON_FAILED", format!("could not build JSON output: {e}"))
        })?;
        ui.always(text);
    } else {
        print_human(context, &report);
    }

    if healthy {
        Ok(())
    } else {
        Err(CliError::usage(
            "DOCTOR_FAILED",
            "something needs fixing before kosong will work properly",
        )
        .with_repair("Each ✖ above says what to do."))
    }
}

fn print_human(context: &Context, report: &Report) {
    let ui = context.ui;

    ui.heading("Checking your setup");
    ui.blank();

    for check in &report.checks {
        ui.status(check.status.mark(), &check.name, &check.detail);
    }

    let needs_action: Vec<&Check> = report
        .checks
        .iter()
        .filter(|c| c.repair.is_some())
        .collect();

    if !needs_action.is_empty() {
        ui.blank();
        ui.heading("What to do");
        for check in needs_action {
            ui.blank();
            ui.say(format!("{}:", check.name));
            for line in check.repair.as_deref().unwrap_or("").lines() {
                ui.say(format!("  {line}"));
            }
        }
    }

    if report.healthy {
        ui.blank();
        ui.say("Everything kosong needs for local work is in place.");
    }

    // §9.2 gives every command one lesson to teach, and this is doctor's. It
    // is said whether or not anything failed: the point is not that today's
    // setup is fine, it is that a setup is a thing you can ask about at all
    // rather than guess at after something breaks.
    ui.blank();
    ui.lesson(
        "Nothing here was guessed. Each line is a question kosong asked your computer,\n\
         and you can ask the same questions yourself at any time.",
    );

    if report.healthy {
        if let Ok(onboarding) = context.onboarding() {
            ui.next_command(onboarding.next_step().command());
        }
    }
}

// ---------------------------------------------------------------------------
// Individual checks
// ---------------------------------------------------------------------------

fn check_config_dir() -> Check {
    match config::config_dir() {
        Ok(dir) => {
            if let Err(e) = std::fs::create_dir_all(&dir) {
                return Check::fail(
                    "settings folder",
                    format!("cannot use {dir}"),
                    format!(
                        "kosong could not create its settings folder ({e}).\n\
                         Check you have permission to write there, or set {}.",
                        config::CONFIG_DIR_ENV
                    ),
                );
            }
            Check::ok("settings folder", dir.as_str())
        }
        Err(error) => Check::fail("settings folder", error.to_string(), error.repair()),
    }
}

fn check_editor(context: &Context) -> Check {
    let setting = context.config().ok().and_then(|c| c.resolve_editor());

    match setting {
        None => Check::warn(
            "editor",
            "not set",
            "kosong needs to know which editor to open.\n\
             export EDITOR=nano\n\
             Add that line to your shell profile to make it permanent.",
        ),
        Some(setting) => match crate::editor::EditorCommand::parse(&setting) {
            Err(error) => Check::warn(
                "editor",
                "cannot be used",
                error.repair.unwrap_or_else(|| error.message.clone()),
            ),
            Ok(None) => Check::warn("editor", "not set", "export EDITOR=nano"),
            Ok(Some(command)) => match tools::find_executable(&command.program) {
                Some(path) => Check::ok("editor", format!("{} ({path})", command.program)),
                None => Check::warn(
                    "editor",
                    format!("`{}` is not installed", command.program),
                    format!(
                        "EDITOR is set to `{}`, but kosong cannot find it.\n\
                         Install it, or choose another:\n  export EDITOR=nano",
                        command.program
                    ),
                ),
            },
        },
    }
}

fn check_document(context: &Context) -> Vec<Check> {
    let Ok(workspace) = context.workspace() else {
        return vec![Check::warn(
            "your page",
            "not made yet",
            "Run: kosong start",
        )];
    };

    if !workspace.has_document() {
        return vec![Check::warn("your page", "not made yet", "Run: kosong new")];
    }

    match workspace.read_document() {
        Ok(document) => {
            let mut checks = vec![Check::ok("your page", workspace.document_path().as_str())];

            // Conformance is reported separately from readability, because a
            // file can be valid OKF while carrying no kosong details.
            checks.push(Check::ok(
                "open knowledge format",
                format!("valid, type `{}`", document.doc_type()),
            ));

            match document.kosong() {
                Ok(Some(_)) => checks.push(Check::ok("kosong details", "present")),
                Ok(None) => checks.push(Check::warn(
                    "kosong details",
                    "not present",
                    "This file follows the Open Knowledge Format but was not made by kosong.\n\
                     Run `kosong status` to let kosong look after it.",
                )),
                Err(error) => checks.push(Check::fail(
                    "kosong details",
                    error.to_string(),
                    error.repair(),
                )),
            }
            checks
        }
        Err(error) => vec![Check::fail("your page", error.to_string(), error.repair())],
    }
}

/// The site folder, when this workspace has one.
///
/// `doctor` runs from the workspace root, and the site folder can sit one level
/// below it. Resolving it is what keeps `doctor` looking where `site publish`
/// looks.
fn site_root(context: &Context) -> Option<camino::Utf8PathBuf> {
    let workspace = context.workspace_or_here().ok()?;
    kosong_core::site::discover(&workspace)
}

fn check_tools(site_root: Option<&camino::Utf8Path>) -> Vec<Check> {
    TOOLS
        .iter()
        .map(|tool| match tools::locate(tool.name, site_root) {
            Some(path) => Check::ok(tool.name, path.as_str()),
            None => Check::warn(
                tool.name,
                "not installed",
                format!(
                    "You do not need this yet. kosong uses it later, to {}.\n{}",
                    tool.purpose, tool.install_hint
                ),
            ),
        })
        .collect()
}

/// Asks `gh` and `wrangler` whether they are signed in.
///
/// This is the one part of `doctor` that runs a child process. Presence is
/// answered by reading PATH; being *signed in* can only be answered by asking
/// the tool. Both calls are read-only allowlisted operations, and a tool that
/// is not installed is skipped rather than run.
/// An operation's invocation, spawning a project-local install where there is
/// one.
///
/// `cwd` stays the workspace root these checks have always run in. Only which
/// binary is spawned changes.
fn resolved_command(
    operation: &dyn Operation,
    cwd: &camino::Utf8Path,
    site_root: Option<&camino::Utf8Path>,
) -> kosong_core::process::SafeCommand {
    let command = operation.command(cwd);
    match site_root.and_then(|root| tools::resolved_program(operation.program(), root)) {
        Some(path) => command.found_at(path),
        None => command,
    }
}

fn check_provider_auth(context: &Context, site_root: Option<&camino::Utf8Path>) -> Vec<Check> {
    let Ok(workspace) = context.workspace_or_here() else {
        return Vec::new();
    };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return Vec::new();
    };

    let mut checks = Vec::new();

    if tools::locate(github::PROGRAM, site_root).is_some() {
        let operation = GitHubOperation::AuthStatus;
        let result =
            runtime.block_on(resolved_command(&operation, workspace.root(), site_root).run());
        checks.push(match result {
            Ok(result) => {
                let state =
                    github::interpret_auth_status(result.exit_code, &result.combined_output());
                match state.repair() {
                    None => Check::ok("github sign-in", "signed in"),
                    Some(repair) => Check::warn("github sign-in", "not signed in", repair),
                }
            }
            Err(error) => Check::warn("github sign-in", "could not check", error.repair()),
        });
    }

    if tools::locate(cloudflare::PROGRAM, site_root).is_some() {
        let operation = CloudflareOperation::WhoAmI;
        let result =
            runtime.block_on(resolved_command(&operation, workspace.root(), site_root).run());
        checks.push(match result {
            Ok(result) => {
                let state =
                    cloudflare::interpret_whoami(result.exit_code, &result.combined_output());
                match state.repair() {
                    None => Check::ok("cloudflare sign-in", "signed in"),
                    Some(repair) => Check::warn("cloudflare sign-in", "not signed in", repair),
                }
            }
            Err(error) => Check::warn("cloudflare sign-in", "could not check", error.repair()),
        });
    }

    checks
}
