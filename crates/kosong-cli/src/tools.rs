//! Finding external tools, without running them.
//!
//! `kosong doctor` needs to know whether `git`, `gh`, `wrangler`, and `npm`
//! are installed. In this phase it answers that by looking at the filesystem
//! only — no child process is started.
//!
//! # Two searches, not one
//!
//! [`find_executable`] searches `PATH`, which is what `git`, `gh`, and `npm`
//! want. [`find_tool`] looks in the project's own `node_modules/.bin` first,
//! which is what Cloudflare's documentation tells people to do with `wrangler`:
//! install it per project so a team shares one pinned version. Before this
//! existed, a user who followed that advice got "wrangler is not installed",
//! and the only hint kosong offered was a global install Cloudflare
//! discourages. Which tools get which search is declared by
//! [`Tool::prefers_local`], not decided at each call site.
//!
//! [`unreachable_install`] answers a third question, asked only once a tool has
//! already not been found: is it installed somewhere `PATH` cannot see? That is
//! the state `npm install -g` leaves behind when the npm prefix's `bin` is not
//! on `PATH` — the command succeeds, and the tool is then missing.
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
    /// Whether a copy in the project's `node_modules/.bin` takes precedence
    /// over `PATH`.
    ///
    /// True only where a project-local install is the documented way to have
    /// the tool. Resolving locally means running a binary the site folder
    /// supplies, and while that folder is already trusted — kosong runs
    /// `npm install` and `npm run build` in it — the widening is kept to the
    /// one tool that earns it rather than handed to `git` for free.
    pub prefers_local: bool,
}

/// The tools `doctor` reports on.
pub const TOOLS: [Tool; 4] = [
    Tool {
        name: "git",
        purpose: "keep the history of your site folder",
        required_for_local: false,
        install_hint: "macOS: xcode-select --install\nLinux: install the `git` package",
        prefers_local: false,
    },
    Tool {
        name: "gh",
        purpose: "create your GitHub repository",
        required_for_local: false,
        install_hint: "See https://cli.github.com  (macOS: brew install gh)",
        prefers_local: false,
    },
    Tool {
        name: "wrangler",
        purpose: "publish your site to Cloudflare",
        required_for_local: false,
        install_hint: "npm install -g wrangler",
        prefers_local: true,
    },
    Tool {
        name: "npm",
        purpose: "build the site template",
        required_for_local: false,
        install_hint: "Install Node.js from https://nodejs.org",
        prefers_local: false,
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

/// Where a Node package manager links a project's own executables.
///
/// npm, pnpm, yarn, and bun all use this path, so a local install made with any
/// of them is found.
const LOCAL_BIN: &str = "node_modules/.bin";

/// Whether a project-local `node_modules/.bin` may be searched on this platform.
///
/// Unix only, deliberately. On Windows that directory holds three files per
/// tool — an extensionless Bourne script, a `.cmd`, and a `.ps1`. The
/// extensionless one is the only candidate [`is_executable`] would accept
/// there, because off Unix it treats any file as runnable; handing that script
/// to `CreateProcess` turns today's clean "not installed" into an
/// unintelligible spawn failure. Running the `.cmd` instead puts `cmd.exe` in
/// the chain, which technical specification §12.1 forbids.
///
/// So Windows keeps the plain `PATH` search. That is a placeholder, not an
/// answer: the §12.1 decision is still open, and resolving it is part of
/// Windows support rather than of this fix.
const LOCAL_RESOLUTION_SUPPORTED: bool = cfg!(unix);

/// Resolves `name` the way a package manager would: the project's own
/// `node_modules/.bin` first, then `PATH`.
///
/// `project_root` is the site folder, which is where `npm install` runs and so
/// where a local install lands. Callers holding a workspace root rather than a
/// site root must resolve the site first — the two are not always the same
/// directory, and searching the wrong one would report a tool missing that
/// `site publish` finds.
pub fn find_tool(name: &str, project_root: &camino::Utf8Path) -> Option<Utf8PathBuf> {
    local_bin_candidate(name, project_root).or_else(|| find_executable(name))
}

/// The project-local candidate for `name`, if there is a usable one.
fn local_bin_candidate(name: &str, project_root: &camino::Utf8Path) -> Option<Utf8PathBuf> {
    // A name with a separator is already a path. `find_executable` checks such
    // a name directly, and looking for it inside a bin directory would be
    // nonsense.
    if !LOCAL_RESOLUTION_SUPPORTED || name.contains('/') {
        return None;
    }

    let candidate = project_root.join(LOCAL_BIN).join(name);
    is_executable(&candidate).then_some(candidate)
}

/// The path a provider operation should spawn for `name`, when that is not the
/// one `PATH` would find.
///
/// `None` means "spawn the allowlisted name and let `PATH` resolve it" — which
/// is what every machine without a project-local install returns, so their
/// invocations are byte-for-byte what they were before this existed. Only a
/// genuine local install produces `Some`, and only for a tool whose
/// [`Tool::prefers_local`] says so. A name absent from [`TOOLS`] is never
/// overridden: the list is the allowlist, and a folder that dropped a `git` into
/// `node_modules/.bin` must not be able to substitute it.
pub fn resolved_program(name: &str, project_root: &camino::Utf8Path) -> Option<Utf8PathBuf> {
    let tool = TOOLS.iter().find(|tool| tool.name == name)?;
    if !tool.prefers_local {
        return None;
    }

    let found = find_tool(name, project_root)?;

    // The `PATH` hit needs no override — spawning the bare name reaches the same
    // binary, and disclosing a path that changes nothing would be noise.
    (find_executable(name).as_ref() != Some(&found)).then_some(found)
}

/// Where kosong looks for `name`, applying whatever local preference it has.
///
/// The one answer to "is this tool here, and which copy is it" — used by the
/// gate that refuses a publish and by `doctor`'s report alike, because the two
/// disagreeing is worse than the bug either would report. `doctor` calling a
/// tool missing while `site publish` finds and runs it sends the user to fix
/// something that is not broken.
///
/// `site_root` is `None` for a workspace that has not run `site init`, which
/// simply means there is nothing local to prefer.
pub fn locate(name: &str, site_root: Option<&camino::Utf8Path>) -> Option<Utf8PathBuf> {
    site_root
        .and_then(|root| resolved_program(name, root))
        .or_else(|| find_executable(name))
}

/// Where `name` is installed, when it is installed somewhere `PATH` cannot see.
///
/// This is the state `npm install -g` leaves behind on a machine whose npm
/// prefix is not on `PATH`: the install reports success, and the tool is then
/// "not found". Naming the path turns a baffling failure into an explicable
/// one.
///
/// # Best-effort, and deliberately so
///
/// This is a diagnostic, not a resolver. It guesses where a global install
/// *would* have gone from two sources — the location of `node`, and an explicit
/// `prefix` in `~/.npmrc` — and neither is authoritative. It is accurate for
/// npm's default prefix, which is the case that produces the failure: Node from
/// nvm, from the nodejs.org installer, or bundled inside an application. It is
/// wrong about Homebrew (it derives the Cellar path rather than the prefix),
/// about shim-based managers like volta and asdf, and about Windows (where the
/// global bin is `%APPDATA%\npm`, not derivable from node's location). In every
/// one of those, the folder is on `PATH` anyway, so the failure does not arise.
///
/// The failure mode is therefore a false negative: nothing is found and the
/// caller falls back to a plain "not installed", which is still correct and
/// still actionable. A false positive is close to impossible, because a path is
/// only ever returned when an executable is genuinely sitting there. Do not
/// mistake this for exhaustive, and do not make a decision depend on its
/// silence.
///
/// The authoritative answer is `npm config get prefix`, which means running
/// npm — and §12.2 permits npm for template install and build only. That
/// widening was considered and declined; a heuristic that is never wrong and
/// sometimes silent is the better trade for a message.
// Called from the publish preflight two phases from now, which is where this
// attribute goes away.
#[allow(dead_code)]
pub fn unreachable_install(name: &str) -> Option<Utf8PathBuf> {
    // Reachable means there is nothing to explain.
    if find_executable(name).is_some() {
        return None;
    }

    unreachable_install_in(&npm_prefixes(), name)
}

/// The search [`unreachable_install`] performs, over a given set of prefixes.
///
/// Split out because the real prefixes come from this machine's `node` and
/// `~/.npmrc`, neither of which a test can stage — and a detector whose only
/// tested outcome is "found nothing" would pass while finding nothing ever.
fn unreachable_install_in(prefixes: &[Utf8PathBuf], name: &str) -> Option<Utf8PathBuf> {
    prefixes
        .iter()
        .map(|prefix| prefix.join("bin"))
        // On `PATH` means reachable, so there is nothing to report. This also
        // keeps the message honest where `find_executable` missed a hit for
        // some other reason: no claim that a directory is absent from `PATH` is
        // made without checking.
        .filter(|dir| !is_on_path(dir))
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

/// Where a global `npm install -g` plausibly put things.
fn npm_prefixes() -> Vec<Utf8PathBuf> {
    let mut prefixes = Vec::new();
    if let Some(prefix) = npm_prefix_from_node() {
        prefixes.push(prefix);
    }
    if let Some(prefix) = npm_prefix_from_npmrc() {
        prefixes.push(prefix);
    }
    prefixes
}

/// npm's default prefix, derived from where `node` actually lives.
///
/// With no `prefix` configured, npm installs global packages into the parent of
/// the directory holding the `node` executable. **Symlinks have to be resolved
/// first.** On the machine this was written for, `PATH` finds
/// `~/.local/bin/node`, whose parent's parent is `~/.local` — but the real node
/// is `~/.hermes/node/bin/node` and the prefix is `~/.hermes/node`, which is
/// where the missing `wrangler` was. Reading the `PATH` entry alone gets the
/// wrong answer and finds nothing.
fn npm_prefix_from_node() -> Option<Utf8PathBuf> {
    let on_path = find_executable("node")?;
    let real = std::fs::canonicalize(&on_path).ok()?;
    let real = Utf8PathBuf::from_path_buf(real).ok()?;

    // <prefix>/bin/node → <prefix>
    Some(real.parent()?.parent()?.to_owned())
}

/// An explicit `prefix=` from the user's `~/.npmrc`.
///
/// Read directly rather than through `npm config get prefix`, which would mean
/// running npm. Only the `prefix` key is looked at — the same file holds auth
/// tokens, and no other value is ever read out of it.
fn npm_prefix_from_npmrc() -> Option<Utf8PathBuf> {
    let text = std::fs::read_to_string(home_dir()?.join(".npmrc")).ok()?;

    text.lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("prefix"))
        // Anchors the key: `prefixed=...` strips to `ed=...`, which has no
        // leading `=` and so is not mistaken for a prefix.
        .filter_map(|rest| rest.trim_start().strip_prefix('='))
        .map(|value| value.trim().trim_matches('"'))
        .find(|value| !value.is_empty())
        .map(Utf8PathBuf::from)
}

fn home_dir() -> Option<Utf8PathBuf> {
    let raw = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Utf8PathBuf::from_path_buf(std::path::PathBuf::from(raw)).ok()
}

/// Whether `dir` is one of the directories `PATH` names.
fn is_on_path(dir: &camino::Utf8Path) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };

    std::env::split_paths(&path_var)
        .filter_map(|entry| Utf8PathBuf::from_path_buf(entry).ok())
        .any(|entry| entry == dir)
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

    /// A throwaway project root. The `TempDir` is returned because dropping it
    /// deletes the directory.
    fn temp_root() -> (tempfile::TempDir, Utf8PathBuf) {
        let guard = tempfile::TempDir::new().expect("temp dir");
        let root = Utf8PathBuf::from_path_buf(guard.path().to_owned()).expect("utf-8 temp path");
        (guard, root)
    }

    /// Writes `node_modules/.bin/<name>` under `root` with the given mode.
    #[cfg(unix)]
    fn local_shim(root: &camino::Utf8Path, name: &str, mode: u32) -> Utf8PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let bin = root.join(LOCAL_BIN);
        std::fs::create_dir_all(&bin).expect("create the local bin");
        let path = bin.join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write the shim");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .expect("chmod the shim");
        path
    }

    #[test]
    #[cfg(unix)]
    fn a_project_local_install_wins_over_one_on_path() {
        // The whole point: `sh` is certainly on PATH, and the local copy must
        // still be the one chosen.
        let (_guard, root) = temp_root();
        let local = local_shim(&root, "sh", 0o755);

        assert_eq!(find_tool("sh", &root), Some(local));
    }

    #[test]
    fn a_tool_with_no_project_local_install_is_found_on_path() {
        let (_guard, root) = temp_root();

        assert_eq!(find_tool("sh", &root), find_executable("sh"));
        assert!(find_tool("sh", &root).is_some());
    }

    #[test]
    #[cfg(unix)]
    fn a_non_executable_in_the_local_bin_is_skipped() {
        // Falling through to PATH beats handing back a file that cannot run.
        let (_guard, root) = temp_root();
        local_shim(&root, "sh", 0o644);

        assert_eq!(find_tool("sh", &root), find_executable("sh"));
    }

    #[test]
    #[cfg(unix)]
    fn a_symlink_to_an_executable_is_accepted() {
        // The shape npm actually writes: `.bin/<name>` is a symlink into the
        // package directory, not a copy. `fs::metadata` follows it, so the mode
        // read is the target's.
        let (_guard, root) = temp_root();
        let bin = root.join(LOCAL_BIN);
        std::fs::create_dir_all(&bin).expect("create the local bin");
        let link = bin.join("kosong-fake-tool");
        std::os::unix::fs::symlink("/bin/sh", &link).expect("symlink the shim");

        assert_eq!(find_tool("kosong-fake-tool", &root), Some(link));
    }

    #[test]
    fn an_explicit_path_is_not_looked_for_in_the_local_bin() {
        let (_guard, root) = temp_root();

        assert_eq!(find_tool("/bin/sh", &root), find_executable("/bin/sh"));
    }

    #[test]
    fn a_reachable_tool_is_not_reported_as_unreachable() {
        // `sh` is on PATH everywhere kosong runs, so there is nothing to
        // explain about it.
        assert_eq!(unreachable_install("sh"), None);
    }

    #[test]
    #[cfg(unix)]
    fn an_install_in_a_directory_off_path_is_reported() {
        // The behaviour the detector exists for, and the one the two `None`
        // cases below cannot show: an executable that is really there, in a
        // directory `PATH` does not name.
        use std::os::unix::fs::PermissionsExt;

        let (_guard, root) = temp_root();
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).expect("create the bin");
        let orphan = bin.join("kosong-fake-tool");
        std::fs::write(&orphan, "#!/bin/sh\nexit 0\n").expect("write the orphan");
        std::fs::set_permissions(&orphan, std::fs::Permissions::from_mode(0o755))
            .expect("chmod the orphan");

        assert_eq!(
            unreachable_install_in(&[root], "kosong-fake-tool"),
            Some(orphan)
        );
    }

    #[test]
    fn a_tool_in_a_directory_on_path_is_not_reported() {
        // Derived rather than hardcoded, so this holds wherever `sh` lives:
        // whatever directory `PATH` found it in is reachable by definition.
        let sh = find_executable("sh").expect("sh is on PATH");
        let prefix = sh
            .parent()
            .and_then(camino::Utf8Path::parent)
            .expect("<prefix>/bin/sh");

        assert_eq!(unreachable_install_in(&[prefix.to_owned()], "sh"), None);
    }

    #[test]
    fn a_tool_installed_nowhere_is_not_reported_as_unreachable() {
        // The detector answers "installed, but out of reach". A tool that is
        // simply absent must not be dressed up as one.
        assert_eq!(
            unreachable_install("kosong-definitely-not-a-real-tool"),
            None
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_tool_that_prefers_local_is_overridden_by_a_local_install() {
        let (_guard, root) = temp_root();
        let local = local_shim(&root, "wrangler", 0o755);

        assert_eq!(resolved_program("wrangler", &root), Some(local));
    }

    #[test]
    #[cfg(unix)]
    fn a_tool_that_does_not_prefer_local_is_never_overridden() {
        // A hostile site folder must not be able to substitute `git` by dropping
        // a file in `node_modules/.bin`.
        let (_guard, root) = temp_root();
        local_shim(&root, "git", 0o755);

        assert_eq!(resolved_program("git", &root), None);
    }

    #[test]
    fn a_tool_with_no_local_install_needs_no_override() {
        // Whether or not wrangler is on this machine's PATH, there is nothing to
        // override: the answer is the one PATH would have given. This is what
        // makes the change inert for everyone without a local install.
        let (_guard, root) = temp_root();

        assert_eq!(resolved_program("wrangler", &root), None);
    }

    #[test]
    #[cfg(unix)]
    fn a_program_outside_the_tool_list_is_never_overridden() {
        // The list is the allowlist. Anything absent from it keeps the plain
        // PATH search rather than picking up whatever the folder supplies.
        let (_guard, root) = temp_root();
        local_shim(&root, "sh", 0o755);

        assert_eq!(resolved_program("sh", &root), None);
    }

    #[test]
    #[cfg(unix)]
    fn locate_prefers_a_local_install_and_falls_back_to_path() {
        let (_guard, root) = temp_root();
        let local = local_shim(&root, "wrangler", 0o755);

        assert_eq!(locate("wrangler", Some(&root)), Some(local));
        // No site folder yet means nothing local to prefer.
        assert_eq!(locate("wrangler", None), find_executable("wrangler"));
        // And a tool with no local copy is still found the ordinary way.
        assert_eq!(locate("sh", Some(&root)), find_executable("sh"));
    }

    #[test]
    fn only_wrangler_prefers_a_project_local_install() {
        // `git`, `gh`, and `npm` stay on a plain PATH search: local resolution
        // means running a binary the site folder supplies, and that widening is
        // only earned where Cloudflare's own documentation sends people.
        for tool in TOOLS {
            assert_eq!(
                tool.prefers_local,
                tool.name == "wrangler",
                "`{}` has the wrong local-install preference",
                tool.name
            );
        }
    }

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
