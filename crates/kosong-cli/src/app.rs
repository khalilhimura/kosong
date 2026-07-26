//! The command tree, per technical specification §5.1.
//!
//! Commands not yet implemented are deliberately absent rather than present
//! and broken. A beginner reading `kosong --help` should see only things that
//! work; discovering that an advertised command is a stub teaches the wrong
//! lesson about whether software can be trusted.

use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "kosong",
    version,
    about = "Write one page. Publish it. Learn what happened.",
    long_about = "kosong helps you make, understand, and publish one Markdown file.\n\n\
                  Everything local works without an account and without a network."
)]
pub struct Cli {
    /// Suppress lessons and success messages. Errors are still shown.
    #[arg(long, global = true)]
    pub quiet: bool,

    /// Folder to work in. Defaults to the current folder.
    #[arg(long, global = true, value_name = "PATH")]
    pub workspace: Option<Utf8PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Set things up, one step at a time.
    Start,

    /// Create a new page.
    New {
        /// Where to write the file.
        #[arg(long, value_name = "PATH")]
        path: Option<Utf8PathBuf>,

        /// The page title.
        #[arg(long, value_name = "TITLE")]
        title: Option<String>,
    },

    /// Open the page in your editor.
    Edit {
        /// Editor to use, just this once.
        #[arg(long, value_name = "COMMAND")]
        editor: Option<String>,

        /// Show which editor would open, without opening it.
        #[arg(long)]
        dry_run: bool,
    },

    /// Show the page and what kosong knows about it.
    Show {
        /// Print the whole file, front matter included.
        #[arg(long)]
        raw: bool,
    },

    /// Serve the page on your own computer.
    Preview {
        /// Port to listen on.
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,
    },

    /// Report the current state of your page and setup.
    Status {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },

    /// Check that everything kosong needs is in place.
    Doctor {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },

    /// Sign in with a code sent to your email.
    Login {
        /// Your email address. You will be asked if this is left out.
        #[arg(long, value_name = "EMAIL")]
        email: Option<String>,

        /// The code from your email, for scripts. Normally you are prompted.
        #[arg(long, value_name = "CODE", requires = "email")]
        code: Option<String>,
    },

    /// Sign out on this computer.
    Logout,

    /// Ask GitHub CLI something about your setup.
    Gh {
        /// What to look up.
        #[arg(value_enum)]
        subcommand: crate::commands::provider::GhSubcommand,

        /// Repository name, for `repo`.
        #[arg(value_name = "OWNER/NAME")]
        name: Option<String>,
    },

    /// Ask Wrangler something about your Cloudflare setup.
    Cf {
        /// What to look up.
        #[arg(value_enum)]
        subcommand: crate::commands::provider::CfSubcommand,
    },

    /// Set up and publish a website for your page.
    #[command(subcommand)]
    Site(SiteCommand),

    /// Keep a private copy of your page on the server.
    Sync {
        /// Send your page to the server.
        #[arg(long, conflicts_with = "pull")]
        push: bool,

        /// Copy the server's page to this computer.
        #[arg(long, conflicts_with = "push")]
        pull: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum SiteCommand {
    /// Turn your page into a publishable folder.
    Init {
        /// Folder and project name. Defaults to your page's title.
        #[arg(value_name = "NAME")]
        name: Option<String>,

        /// Show what would happen, without doing it.
        #[arg(long)]
        dry_run: bool,

        /// Do not ask before making changes.
        #[arg(long)]
        yes: bool,
    },

    /// Build your page and put it online.
    Publish {
        /// Show what would happen, without doing it.
        #[arg(long)]
        dry_run: bool,

        /// Do not ask before making changes.
        #[arg(long)]
        yes: bool,
    },

    /// See past versions, and how to go back to one.
    Rollback {
        /// Show what would happen, without doing it.
        #[arg(long)]
        dry_run: bool,

        /// Do not ask before making changes.
        #[arg(long)]
        yes: bool,
    },
}
