//! `kosong` — write one page, publish it, learn what happened.

mod app;
mod commands;
mod editor;
mod exit;
mod remote;
mod tools;
mod ui;

use app::{Cli, Command};
use clap::Parser;
use commands::Context;
use exit::{CliResult, Exit};
use ui::Ui;

/// Restores the default `SIGPIPE` behaviour.
///
/// Rust ignores `SIGPIPE` at startup, so writing to a closed pipe returns
/// `EPOLL`/`EPIPE` and `println!` panics. That turns `kosong show | head` —
/// an entirely ordinary thing to type — into a crash with a backtrace.
///
/// Every other Unix tool dies quietly here, and so should this one.
#[cfg(unix)]
fn restore_sigpipe() {
    // SAFETY: setting a signal disposition to the default is async-signal-safe
    // and is done once, before any threads exist.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_sigpipe() {}

fn main() -> std::process::ExitCode {
    restore_sigpipe();

    let cli = Cli::parse();

    // JSON output must be machine-readable, so colour is suppressed for it
    // regardless of whether stdout is a terminal.
    let wants_json = matches!(
        cli.command,
        Command::Status { json: true } | Command::Doctor { json: true }
    );
    let ui = if wants_json {
        Ui::plain(cli.quiet)
    } else {
        Ui::new(cli.quiet)
    };

    match run(&cli, ui) {
        Ok(()) => std::process::ExitCode::from(Exit::Success.code() as u8),
        Err(error) => {
            ui.error(&error.message, error.repair.as_deref());
            std::process::ExitCode::from(error.exit.code() as u8)
        }
    }
}

fn run(cli: &Cli, ui: Ui) -> CliResult<()> {
    let context = Context::new(ui, cli.workspace.clone())?;

    match &cli.command {
        Command::Start => commands::start::run(&context),

        Command::New { path, title } => commands::new::run(&context, path.clone(), title.clone()),

        Command::Edit { editor, dry_run } => {
            commands::edit::run(&context, editor.clone(), *dry_run)
        }

        Command::Show { raw } => commands::show::run(&context, *raw),

        Command::Preview { port } => commands::preview::run(&context, *port),

        Command::Status { json } => commands::status::run(&context, *json),

        Command::Doctor { json } => commands::doctor::run(&context, *json),

        Command::Login { email, code } => {
            commands::login::run(&context, email.clone(), code.clone())
        }

        Command::Logout => commands::login::logout(&context),

        Command::DeleteAccount { dry_run, yes } => {
            commands::account::delete(&context, *dry_run, *yes)
        }

        Command::Gh { subcommand, name } => {
            commands::provider::run_gh(&context, *subcommand, name.clone())
        }

        Command::Cf { subcommand } => commands::provider::run_cf(&context, *subcommand),

        Command::Site(site) => {
            use app::SiteCommand;
            use commands::site::Approval;

            match site {
                SiteCommand::Init { name, dry_run, yes } => commands::site::init(
                    &context,
                    name.clone(),
                    Approval {
                        dry_run: *dry_run,
                        assume_yes: *yes,
                    },
                ),
                SiteCommand::Publish { dry_run, yes } => commands::site::publish(
                    &context,
                    Approval {
                        dry_run: *dry_run,
                        assume_yes: *yes,
                    },
                ),
                SiteCommand::Rollback { dry_run, yes } => commands::site::rollback(
                    &context,
                    Approval {
                        dry_run: *dry_run,
                        assume_yes: *yes,
                    },
                ),
            }
        }

        Command::Sync { push, pull } => {
            use commands::sync::Direction;
            let direction = match (push, pull) {
                (true, _) => Direction::Push,
                (_, true) => Direction::Pull,
                _ => Direction::Detect,
            };
            commands::sync::run(&context, direction)
        }
    }
}
