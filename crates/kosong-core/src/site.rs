//! The generated site: scaffolding, state, and preparing content to publish.
//!
//! # The template is compiled in
//!
//! §13.1 requires the template be bundled or fetched from a checksum-verified
//! release, and says it "must never clone/download a moving `main` branch at
//! runtime". The files are therefore `include_str!`d into the binary. A given
//! `kosong` build always writes exactly the template it was built with, and
//! scaffolding works offline.
//!
//! # Content is rendered here, not in the template
//!
//! The template contains no Markdown parser. `kosong` renders the document
//! with [`crate::render`] — the same code and the same tests behind
//! `kosong preview` — and writes safe HTML for the template to inject.
//!
//! This is a security property, not a convenience. Rendering Markdown a second
//! time in JavaScript would mean two implementations of one policy, and the
//! moment they drift a page can look safe in preview and publish a live
//! `<script>`. That drift was real: an earlier template did parse Markdown
//! itself, and raw HTML survived into the built output.

use crate::okf::Document;
use crate::render;
use crate::workspace::{self, Workspace, WorkspaceError};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

/// Directory name for generated content inside the site.
const CONTENT_DIR: &str = "src/content";

/// npm's lockfile, written into the site by `npm install`.
const LOCKFILE: &str = "package-lock.json";

/// Where the built output lands. Fixed by the template's Astro config.
pub const BUILD_OUTPUT_DIR: &str = "dist";

/// Filename for site state within `.kosong`.
const STATE_FILENAME: &str = "site.toml";

/// One file of the bundled template.
pub struct TemplateFile {
    /// Path relative to the site root.
    pub path: &'static str,
    pub contents: &'static str,
}

/// The template, compiled into the binary.
///
/// `src/content/page.html` and `src/content/page.json` are absent on purpose:
/// they are generated from the user's document, and shipping placeholders that
/// a failed publish could leave behind would be worse than having none.
pub const TEMPLATE: [TemplateFile; 4] = [
    TemplateFile {
        path: "package.json",
        contents: include_str!("../../../templates/site/package.json"),
    },
    TemplateFile {
        path: "astro.config.mjs",
        contents: include_str!("../../../templates/site/astro.config.mjs"),
    },
    TemplateFile {
        path: "src/pages/index.astro",
        contents: include_str!("../../../templates/site/src/pages/index.astro"),
    },
    TemplateFile {
        path: ".gitignore",
        contents: include_str!("../../../templates/site/.gitignore"),
    },
];

/// Paths `kosong` owns and may stage for commit.
///
/// Used both for scaffolding and for the §13.3 dirty-tree check. A file
/// outside this list belongs to the user and is never committed by kosong.
///
/// # Owned is not the same as written by kosong
///
/// `package-lock.json` is here but deliberately not in [`TEMPLATE`]. npm writes
/// it, not kosong, and it has no contents to `include_str!` — putting it in the
/// template would make `scaffold` create an empty lockfile at `site init`,
/// which is a lie about a file npm is about to generate properly.
///
/// It has to be owned all the same. `npm install` runs at publish step 4 and
/// leaves the lockfile behind, so without this every *second* publish saw a
/// file it had not written, called it unrelated, and refused: `init` →
/// `publish` ✓ → `publish` ✗, for every site. Owning it also keeps the promise
/// the product is built on — an ordinary Astro project you own is materially
/// weaker without the lockfile, because the build is not reproducible without
/// one.
///
/// # These paths are not guaranteed to exist
///
/// The lockfile is absent for the whole of `site init`. Callers handing this
/// list to `git add` or `git commit --only` must filter it to what is actually
/// there — see `present` in the CLI's `site` command.
pub fn owned_paths() -> Vec<Utf8PathBuf> {
    let mut paths: Vec<Utf8PathBuf> = TEMPLATE
        .iter()
        .map(|file| Utf8PathBuf::from(file.path))
        .collect();
    paths.push(Utf8PathBuf::from(CONTENT_DIR));
    paths.push(Utf8PathBuf::from(LOCKFILE));
    paths
}

#[derive(Debug, thiserror::Error)]
pub enum SiteError {
    #[error("this folder is not set up as a site yet")]
    NotInitialized,

    #[error("a site already exists at `{path}`")]
    AlreadyExists { path: Utf8PathBuf },

    #[error("the build produced no `{BUILD_OUTPUT_DIR}` folder")]
    NoBuildOutput,

    #[error("could not read the site's settings: {0}")]
    Malformed(String),

    #[error(transparent)]
    Storage(#[from] WorkspaceError),
}

impl SiteError {
    pub fn repair(&self) -> String {
        match self {
            Self::NotInitialized => "Run: kosong site init".into(),
            Self::AlreadyExists { .. } => {
                "Run `kosong site publish` to update it, or choose another name.".into()
            }
            Self::NoBuildOutput => {
                "The build finished but wrote nothing. Run `kosong site publish` again, and if it \
                 keeps happening, please report it."
                    .into()
            }
            Self::Malformed(_) => {
                "Delete `.kosong/site.toml` and run `kosong site init` again.".into()
            }
            Self::Storage(e) => e.repair(),
        }
    }
}

/// Non-secret state for a site, stored in `.kosong/site.toml`.
///
/// Contains names and paths only. No tokens, no account identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteState {
    pub version: u32,
    /// The document this site publishes, relative to the site root.
    pub source_document: Utf8PathBuf,
    pub site_name: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloudflare_project: Option<String>,
    /// URL of the last successful publish.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_deployment_url: Option<String>,
}

impl SiteState {
    pub fn new(site_name: String, source_document: Utf8PathBuf) -> Self {
        Self {
            version: 1,
            source_document,
            cloudflare_project: Some(site_name.clone()),
            site_name,
            github_repo: None,
            last_deployment_url: None,
        }
    }

    pub fn path(site_root: &Utf8Path) -> Utf8PathBuf {
        site_root.join(".kosong").join(STATE_FILENAME)
    }

    pub fn load(site_root: &Utf8Path) -> Result<Self, SiteError> {
        let text = std::fs::read_to_string(Self::path(site_root))
            .map_err(|_| SiteError::NotInitialized)?;
        toml::from_str(&text).map_err(|e| SiteError::Malformed(e.to_string()))
    }

    pub fn save(&self, site_root: &Utf8Path) -> Result<(), SiteError> {
        let text = toml::to_string_pretty(self).map_err(|e| SiteError::Malformed(e.to_string()))?;
        workspace::atomic_write(&Self::path(site_root), text.as_bytes())?;
        Ok(())
    }

    /// The Cloudflare project name, falling back to the site name.
    pub fn project(&self) -> &str {
        self.cloudflare_project
            .as_deref()
            .unwrap_or(&self.site_name)
    }
}

/// Writes the bundled template into `site_root`.
///
/// Refuses to overwrite an existing site: `site init` is not a reset, and
/// silently replacing a customised `index.astro` would discard the user's work.
pub fn scaffold(site_root: &Utf8Path) -> Result<Vec<Utf8PathBuf>, SiteError> {
    if SiteState::path(site_root).exists() {
        return Err(SiteError::AlreadyExists {
            path: site_root.to_owned(),
        });
    }

    let mut written = Vec::new();
    for file in &TEMPLATE {
        let target = site_root.join(file.path);
        workspace::atomic_write(&target, file.contents.as_bytes())?;
        written.push(Utf8PathBuf::from(file.path));
    }
    Ok(written)
}

/// What was written when preparing content for a build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedContent {
    pub html_path: Utf8PathBuf,
    pub meta_path: Utf8PathBuf,
    /// Anything the safety policy removed, for telling the user.
    pub removal_note: Option<String>,
}

/// Renders the document into the template's content files.
///
/// Called before every build, so the published page always reflects the
/// current document rather than whatever was there last time.
pub fn prepare_content(
    site_root: &Utf8Path,
    document: &Document,
    filename: &str,
) -> Result<PreparedContent, SiteError> {
    let rendered = render::render_markdown(document.body());

    let html_path = site_root.join(CONTENT_DIR).join("page.html");
    workspace::atomic_write(&html_path, rendered.html.as_bytes())?;

    let meta = PageMeta {
        title: document.title_or_filename(filename),
        description: document.description().unwrap_or_default().to_owned(),
    };
    let meta_json =
        serde_json::to_string_pretty(&meta).map_err(|e| SiteError::Malformed(e.to_string()))?;

    let meta_path = site_root.join(CONTENT_DIR).join("page.json");
    workspace::atomic_write(&meta_path, meta_json.as_bytes())?;

    Ok(PreparedContent {
        html_path,
        meta_path,
        removal_note: rendered.removal_note(),
    })
}

#[derive(Debug, Serialize)]
struct PageMeta {
    title: String,
    description: String,
}

/// Whether a build produced usable output.
///
/// §13.3 step 6. A build tool can exit zero having written nothing, and
/// deploying an empty directory would replace a working site with a blank one.
pub fn build_output_exists(site_root: &Utf8Path) -> bool {
    let dist = site_root.join(BUILD_OUTPUT_DIR);
    dist.is_dir() && dist.join("index.html").is_file()
}

/// Derives a site folder name from a title or an explicit name.
pub fn site_name_from(preferred: Option<&str>, document_title: &str) -> String {
    // A blank `--name` is the same as not giving one. Treating `Some("")` as a
    // real choice would skip the title and land everyone on the fallback.
    let candidate = preferred
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| crate::okf::slugify(document_title));

    let slug = crate::okf::slugify(&candidate);
    if slug.is_empty() {
        "my-site".to_owned()
    } else {
        slug
    }
}

/// Finds the site folder belonging to a workspace, if one exists.
pub fn discover(workspace: &Workspace) -> Option<Utf8PathBuf> {
    let root = workspace.root();

    // The site folder sits beside the document, so the workspace root itself
    // is checked first, then one level down.
    if SiteState::path(root).exists() {
        return Some(root.to_owned());
    }

    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = Utf8PathBuf::from_path_buf(entry.path()).ok()?;
        if path.is_dir() && SiteState::path(&path).exists() {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_is_compiled_in_and_complete() {
        // §13.1 names the required paths.
        let paths: Vec<&str> = TEMPLATE.iter().map(|f| f.path).collect();

        assert!(paths.contains(&"package.json"));
        assert!(paths.contains(&"astro.config.mjs"));
        assert!(paths.contains(&"src/pages/index.astro"));

        for file in &TEMPLATE {
            assert!(!file.contents.is_empty(), "`{}` is empty", file.path);
        }
    }

    #[test]
    fn the_template_pins_an_exact_astro_version() {
        // §12.2 permits npm only with a version-pinned template. A range would
        // mean two machines building different sites from the same document.
        let package = TEMPLATE
            .iter()
            .find(|f| f.path == "package.json")
            .expect("package.json");

        assert!(package.contents.contains("\"astro\": \""));
        assert!(
            !package.contents.contains("^") && !package.contents.contains("~"),
            "dependency versions must be exact, not ranges:\n{}",
            package.contents
        );
    }

    #[test]
    fn the_template_parses_no_markdown_of_its_own() {
        // The security property: one renderer, in Rust. A Markdown parser in
        // the template would be a second implementation of the safety policy.
        let page = TEMPLATE
            .iter()
            .find(|f| f.path == "src/pages/index.astro")
            .expect("index.astro");

        // Checks for imports, not for the words: the file's own comment
        // explains why those parsers were removed, and that prose is worth
        // keeping.
        assert!(!page.contents.contains("from \"marked\""));
        assert!(!page.contents.contains("from \"front-matter\""));
        assert!(page.contents.contains("page.html"));

        // The pinned manifest must not carry them either.
        let package = TEMPLATE
            .iter()
            .find(|f| f.path == "package.json")
            .expect("package.json");
        assert!(!package.contents.contains("marked"));
        assert!(!package.contents.contains("front-matter"));
    }

    #[test]
    fn owned_paths_cover_the_template_and_generated_content() {
        let owned = owned_paths();

        assert!(owned.contains(&Utf8PathBuf::from("package.json")));
        assert!(owned.contains(&Utf8PathBuf::from("src/content")));
        assert!(!owned.contains(&Utf8PathBuf::from("node_modules")));
        assert!(!owned.contains(&Utf8PathBuf::from("dist")));

        // Owned, because `npm install` leaves it in the site at publish step 4
        // and the next publish would otherwise read it as a stray file.
        assert!(owned.contains(&Utf8PathBuf::from("package-lock.json")));

        // But not a template file: kosong has nothing to write there, and
        // scaffolding an empty lockfile would be worse than having none.
        assert!(
            !TEMPLATE.iter().any(|f| f.path == "package-lock.json"),
            "the lockfile is npm's to write, not kosong's"
        );
    }

    #[test]
    fn the_lockfile_npm_leaves_behind_is_not_a_change_kosong_did_not_make() {
        // The bug this guards, seen in live smoke run 30335517886: the first
        // publish ran `npm install`, which wrote `package-lock.json`; the
        // second publish read that file as unrelated and refused, telling the
        // user to commit a file kosong itself had caused to appear.
        //
        // Written against `unrelated_changes` rather than the list alone
        // because the refusal is what the user actually hits, and the prefix
        // matching in between is where an entry like `package` would still
        // have looked correct in `owned_paths()` while failing here.
        let status = "?? package-lock.json\n?? my-private-notes.txt\n";

        let unrelated = crate::providers::git::unrelated_changes(status, &owned_paths());
        let paths: Vec<&str> = unrelated.iter().map(|c| c.path.as_str()).collect();

        assert_eq!(
            paths,
            ["my-private-notes.txt"],
            "the lockfile is ours; the notes file is not"
        );
    }

    #[test]
    fn an_owned_directory_counts_as_tracked_when_git_lists_what_is_inside_it() {
        // `git ls-files -- src/content` answers with the files under it, never
        // with the directory, so an exact match alone would read the one owned
        // directory as untracked — and a publish would then filter it out the
        // moment it was deleted, which is exactly the case worth staging.
        //
        // Written against `owned_paths` for the same reason the lockfile test
        // is: the prefix matching in between is where a near-miss entry stays
        // invisible.
        let listed = "package.json\nsrc/content/page.html\nsrc/content/page.json\n";
        let tracked: Vec<String> = owned_paths()
            .iter()
            .filter(|path| crate::providers::git::tracks(listed, path))
            .map(ToString::to_string)
            .collect();

        assert!(tracked.iter().any(|path| path == "src/content"));
        assert!(tracked.iter().any(|path| path == "package.json"));
        assert!(
            !tracked.iter().any(|path| path == "astro.config.mjs"),
            "a path git did not list is not tracked"
        );
        assert!(
            !tracked.iter().any(|path| path == "package-lock.json"),
            "`package.json` being listed must not make the lockfile tracked"
        );
    }

    #[test]
    fn a_site_name_is_always_url_safe() {
        assert_eq!(site_name_from(Some("My Site"), "ignored"), "my-site");
        assert_eq!(site_name_from(None, "My First Site"), "my-first-site");
        // Non-ASCII slugifies to nothing, so a usable fallback is needed.
        assert_eq!(site_name_from(None, "日本語"), "my-site");
        assert_eq!(site_name_from(Some(""), "Fallback Title"), "fallback-title");
    }

    #[test]
    fn state_round_trips_and_holds_no_secret() {
        let state = SiteState::new("my-site".into(), Utf8PathBuf::from("../kosong.md"));
        let text = toml::to_string_pretty(&state).unwrap();

        let parsed: SiteState = toml::from_str(&text).unwrap();
        assert_eq!(parsed, state);
        assert_eq!(parsed.project(), "my-site");

        for forbidden in ["token", "secret", "password", "@"] {
            assert!(
                !text.to_lowercase().contains(forbidden),
                "site state must never contain `{forbidden}`"
            );
        }
    }
}
