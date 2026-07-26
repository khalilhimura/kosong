//! npm, for building the site template.
//!
//! §12.2 permits `npm` for "template install/build only with a version-pinned
//! template and known working directory". Both conditions hold here: the
//! template's `package.json` pins Astro to an exact version, and the working
//! directory is always the site root that `site init` created.
//!
//! There is no variant for running an arbitrary script. `npm run build` is
//! spelled out, not parameterised.

use super::Operation;
use camino::Utf8PathBuf;

/// The executable. Fixed, never derived from input.
pub const PROGRAM: &str = "npm";

/// A permitted npm operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NpmOperation {
    /// Fetch the template's pinned dependencies.
    Install,
    /// Run the template's build script.
    Build,
}

impl Operation for NpmOperation {
    fn program(&self) -> &'static str {
        PROGRAM
    }

    fn args(&self) -> Vec<String> {
        match self {
            Self::Install => vec![
                "install".into(),
                // Nothing in the template needs a build step of its own, and
                // refusing to run install scripts removes a large class of
                // supply-chain surprise from a command a beginner is told to
                // trust.
                "--ignore-scripts".into(),
                // No progress bar or funding notices: the output is shown to
                // someone who is not expecting a wall of npm chatter.
                "--no-fund".into(),
                "--no-audit".into(),
            ],
            Self::Build => vec!["run".into(), "build".into()],
        }
    }

    fn mutating(&self) -> bool {
        // Both write to the site folder: `node_modules` and `dist`. Neither
        // touches anything outside it.
        true
    }

    fn summary(&self) -> String {
        match self {
            Self::Install => "Fetch the pieces needed to build your page.".into(),
            Self::Build => "Turn your page into files a website can serve.".into(),
        }
    }

    fn remote_effect(&self) -> Option<String> {
        // Install downloads, but publishes nothing. The distinction the user
        // cares about is whether anything of theirs leaves the machine.
        None
    }

    fn files(&self) -> Vec<Utf8PathBuf> {
        match self {
            Self::Install => vec![Utf8PathBuf::from("node_modules")],
            Self::Build => vec![Utf8PathBuf::from("dist")],
        }
    }

    fn timeout(&self) -> std::time::Duration {
        crate::process::LONG_TIMEOUT
    }
}
