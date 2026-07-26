//! Finding external tools on `PATH`, without running them.
//!
//! `kosong doctor` needs to know whether `git`, `gh`, `wrangler`, and `npm`
//! are installed. In this phase it answers that by looking at the filesystem
//! only — no child process is started.
//!
//! That matters for two reasons. Running an unknown binary just to see whether
//! it exists is a worse trade than reading a directory entry. And `doctor` is
//! the command a confused user runs, which is exactly when executing arbitrary
//! things found on `PATH` is least appealing.
//!
//! Checking whether a tool is *signed in* does require running it, and arrives
//! with the process adapters in a later phase.

use camino::Utf8PathBuf;

/// An external tool `kosong` can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tool {
    /// Executable name as invoked.
    pub name: &'static str,
    /// What `kosong` uses it for, in plain language.
    pub purpose: &'static str,
    /// Whether local work is possible without it.
    pub required_for_local: bool,
    /// How to install it.
    pub install_hint: &'static str,
}

/// The tools `doctor` reports on.
pub const TOOLS: [Tool; 4] = [
    Tool {
        name: "git",
        purpose: "keep the history of your site folder",
        required_for_local: false,
        install_hint: "macOS: xcode-select --install\nLinux: install the `git` package",
    },
    Tool {
        name: "gh",
        purpose: "create your GitHub repository",
        required_for_local: false,
        install_hint: "See https://cli.github.com  (macOS: brew install gh)",
    },
    Tool {
        name: "wrangler",
        purpose: "publish your site to Cloudflare",
        required_for_local: false,
        install_hint: "npm install -g wrangler",
    },
    Tool {
        name: "npm",
        purpose: "build the site template",
        required_for_local: false,
        install_hint: "Install Node.js from https://nodejs.org",
    },
];

/// Looks up `name` on `PATH`, returning the first executable match.
///
/// An absolute or relative path containing a separator is checked directly
/// rather than searched for, matching how a shell resolves such a name.
pub fn find_executable(name: &str) -> Option<Utf8PathBuf> {
    if name.contains('/') {
        let path = Utf8PathBuf::from(name);
        return is_executable(&path).then_some(path);
    }

    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .filter_map(|dir| Utf8PathBuf::from_path_buf(dir).ok())
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

/// Whether `path` is a file the current user could execute.
///
/// On Unix this checks the executable bits. Elsewhere, being a file is taken
/// as sufficient.
fn is_executable(path: &camino::Utf8Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_that_exists_everywhere_is_found() {
        // `sh` is present on every supported platform. This asserts the search
        // works, not that kosong uses sh — it does not.
        assert!(find_executable("sh").is_some());
    }

    #[test]
    fn a_tool_that_does_not_exist_is_not_found() {
        assert_eq!(find_executable("kosong-definitely-not-a-real-tool"), None);
    }

    #[test]
    fn a_directory_is_not_an_executable() {
        assert!(!is_executable(camino::Utf8Path::new("/tmp")));
    }

    #[test]
    fn an_explicit_path_is_checked_directly() {
        assert!(find_executable("/bin/sh").is_some());
        assert!(find_executable("/bin/definitely-not-here").is_none());
    }

    #[test]
    fn every_tool_explains_itself() {
        for tool in TOOLS {
            assert!(!tool.purpose.is_empty());
            assert!(!tool.install_hint.is_empty());
            // Local-first: nothing external may be required for local work.
            assert!(
                !tool.required_for_local,
                "`{}` must not be required for local use",
                tool.name
            );
        }
    }
}
