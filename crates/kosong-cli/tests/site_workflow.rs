//! The site workflow, driven against fake `git`, `gh`, `npm`, and `wrangler`.
//!
//! §15 requires `site init/publish --dry-run` be tested against fake binaries.
//! Real ones would need a GitHub account and a Cloudflare account, so the CI
//! path uses stand-ins that record what they were asked to do.
//!
//! Each fake writes its argv to a log file, which is how these tests assert
//! that a `--dry-run` invoked *nothing* — the strongest form of that claim is
//! an empty log.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;
use tempfile::TempDir;

fn binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("test binary path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("kosong")
}

// ---------------------------------------------------------------------------
// Exec'ing a fake this test just wrote
// ---------------------------------------------------------------------------

/// How many times a fake gets to be busy before we believe it.
const EXEC_ATTEMPTS: usize = 10;

/// Long enough for a forked child to reach its own exec, short enough that a
/// genuine failure is still reported promptly.
const EXEC_RETRY_PAUSE: Duration = Duration::from_millis(20);

/// Runs `attempt`, retrying only while it fails with `ETXTBSY`.
///
/// Tests run on parallel threads, and glibc's `posix_spawn` clones a child
/// with a copy of the descriptor table but not `CLONE_FILES`. A child forked
/// while `Sandbox::fake` still holds a fake open for writing keeps that
/// descriptor until its own exec clears it, and exec'ing a file another
/// process holds open for writing is `ETXTBSY`.
///
/// The condition is transient by construction: it lasts until that child
/// execs. Retries that run out return the original error rather than
/// swallowing it, and any other failure is returned on the first attempt.
fn with_busy_retry<T>(mut attempt: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut outcome = attempt();

    for _ in 1..EXEC_ATTEMPTS {
        match &outcome {
            Err(error) if error.kind() == ErrorKind::ExecutableFileBusy => {}
            _ => break,
        }
        std::thread::sleep(EXEC_RETRY_PAUSE);
        outcome = attempt();
    }

    outcome
}

#[test]
fn a_busy_fake_is_waited_out_rather_than_reported() {
    // The race cannot be provoked on demand, so the policy is pinned directly.
    let mut attempts = 0;

    let outcome = with_busy_retry(|| {
        attempts += 1;
        if attempts < 3 {
            Err(std::io::Error::from(ErrorKind::ExecutableFileBusy))
        } else {
            Ok(())
        }
    });

    assert!(outcome.is_ok(), "must succeed once the fake is free");
    assert_eq!(attempts, 3);
}

#[test]
fn a_fake_that_stays_busy_is_reported_in_the_end() {
    // Retrying must not turn a real, persistent failure into silence.
    let mut attempts = 0;

    let outcome: std::io::Result<()> = with_busy_retry(|| {
        attempts += 1;
        Err(std::io::Error::from(ErrorKind::ExecutableFileBusy))
    });

    assert_eq!(
        outcome.unwrap_err().kind(),
        ErrorKind::ExecutableFileBusy,
        "the original error must survive"
    );
    assert_eq!(attempts, EXEC_ATTEMPTS);
}

#[test]
fn a_fake_that_fails_for_another_reason_is_reported_at_once() {
    // A fake that is missing is not a race, and waiting to say so helps nobody.
    let mut attempts = 0;

    let outcome: std::io::Result<()> = with_busy_retry(|| {
        attempts += 1;
        Err(std::io::Error::from(ErrorKind::NotFound))
    });

    assert_eq!(outcome.unwrap_err().kind(), ErrorKind::NotFound);
    assert_eq!(attempts, 1);
}

/// A workspace plus a directory of fake provider tools.
struct Sandbox {
    _guard: TempDir,
    root: PathBuf,
    config: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let guard = TempDir::new().expect("temp dir");
        let root = std::fs::canonicalize(guard.path()).expect("canonicalize");
        let config = root.join("config");
        let bin = root.join("fakebin");
        let log = root.join("invocations.log");

        std::fs::create_dir_all(&config).expect("config dir");
        std::fs::create_dir_all(&bin).expect("bin dir");

        let sandbox = Self {
            _guard: guard,
            root,
            config,
            bin,
            log,
        };
        sandbox.install_fakes();
        sandbox
    }

    /// Writes a fake tool that logs its argv and exits successfully.
    fn fake(&self, name: &str, extra: &str) {
        let path = self.bin.join(name);
        let body = format!(
            "#!/bin/sh\nprintf '{name} %s\\n' \"$*\" >> '{log}'\n{extra}\nexit 0\n",
            name = name,
            log = self.log.display(),
            extra = extra,
        );
        std::fs::write(&path, body).expect("write fake");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod fake");
        }

        #[cfg(target_os = "linux")]
        self.wait_until_runnable(&path);
    }

    /// Runs a fake once, here, before kosong is ever asked to.
    ///
    /// Unlike the core process tests, nothing in this file execs a fake
    /// directly — kosong does, from a child process, so an `ETXTBSY` would
    /// surface as an unrecognisable kosong failure with no way to tell it from
    /// a real one. Running the fake once here gives that error somewhere to be
    /// caught, and settles the question for good: once a fake has run, no
    /// process holds it open for writing, and nothing writes it again.
    ///
    /// Linux only, because that is where the window exists. macOS `posix_spawn`
    /// is a single kernel syscall with no forked child to inherit a descriptor,
    /// and 2400 write-then-exec races there produced no `ETXTBSY` at all. It
    /// would not be a free precaution either: the first exec of a newly written
    /// file on macOS costs ~150ms of Gatekeeper assessment, which across every
    /// fake of every sandbox added 14s to this binary.
    #[cfg(target_os = "linux")]
    fn wait_until_runnable(&self, path: &Path) {
        // The probe is an invocation like any other, so the fake logs it. No
        // test asked for that line, so the log is wound back afterwards.
        let logged = std::fs::metadata(&self.log).map(|m| m.len()).unwrap_or(0);

        let outcome = with_busy_retry(|| {
            Command::new(path)
                .current_dir(&self.root)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|_| ())
        });

        if let Err(error) = outcome {
            panic!("the fake `{}` cannot be run: {error}", path.display());
        }

        // The probe has run, so the fake's own `>>` has created the log: this
        // cannot be the case where there is nothing to wind back. Failing here
        // rather than shrugging keeps the stray line from turning up later as
        // an empty-log assertion failing in an unrelated test.
        let log = std::fs::OpenOptions::new()
            .write(true)
            .open(&self.log)
            .expect("reopen the log to wind it back");
        log.set_len(logged).expect("wind the log back");
    }

    fn install_fakes(&self) {
        // git must behave enough for the workflow: `status --porcelain` clean,
        // `config --get user.email` returning an identity.
        self.fake(
            "git",
            r#"case "$1" in
  status) exit 0 ;;
  config) echo "tester@example.test" ;;
  rev-parse) echo "main" ;;
esac"#,
        );
        self.fake("gh", r#"[ "$1" = "auth" ] && exit 0"#);
        // npm build must produce the output the publish step checks for.
        self.fake(
            "npm",
            r#"if [ "$1" = "run" ] && [ "$2" = "build" ]; then
  mkdir -p dist && echo '<!doctype html><title>fake</title>' > dist/index.html
fi"#,
        );
        self.fake(
            "wrangler",
            r#"echo "Deployment complete! Take a peek over at https://abc123.my-first-site.pages.dev""#,
        );
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(binary())
            .arg("--workspace")
            .arg(&self.root)
            .args(args)
            .env("KOSONG_CONFIG_DIR", &self.config)
            .env("KOSONG_SESSION_FILE", self.config.join("session"))
            // The fakes come first, so they shadow any real provider. The
            // system directories follow because the fakes are shell scripts
            // and need `mkdir` and friends — without them a fake fails with
            // "command not found" and looks like a kosong bug.
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("NO_COLOR", "1")
            .output()
            .expect("run kosong")
    }

    /// Everything the fakes were asked to do.
    fn invocations(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn site_root(&self) -> PathBuf {
        self.root.join("my-first-site")
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
fn code(output: &Output) -> i32 {
    output.status.code().expect("exited normally")
}

// ---------------------------------------------------------------------------
// site init
// ---------------------------------------------------------------------------

#[test]
fn init_writes_the_template_and_starts_a_repository() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new", "--title", "My First Site"]);

    let output = sandbox.run(&["site", "init", "--yes"]);
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));

    for file in [
        "package.json",
        "astro.config.mjs",
        "src/pages/index.astro",
        ".gitignore",
        "src/content/page.html",
        "src/content/page.json",
        ".kosong/site.toml",
    ] {
        assert!(
            sandbox.site_root().join(file).is_file(),
            "`{file}` was not written"
        );
    }

    let log = sandbox.invocations();
    assert!(log.contains("git init"));
    assert!(log.contains("git commit"));
}

#[test]
fn init_stages_only_the_files_kosong_wrote() {
    // §12.2: git is called "with known file paths only". A user's stray file
    // must never be swept into a commit.
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);
    sandbox.run(&["site", "init", "--yes"]);

    let log = sandbox.invocations();
    let add_line = log
        .lines()
        .find(|line| line.starts_with("git add"))
        .expect("a git add invocation");

    assert!(add_line.contains("package.json"));
    assert!(add_line.contains("src/content"));
    // Never a catch-all.
    assert!(!add_line.contains(" . "));
    assert!(!add_line.contains("-A"));
    assert!(!add_line.contains("--all"));
}

#[test]
fn init_refuses_to_overwrite_an_existing_site() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);
    sandbox.run(&["site", "init", "--yes"]);

    // Customise a file, then re-init.
    let page = sandbox.site_root().join("src/pages/index.astro");
    std::fs::write(&page, "-- my own version --").unwrap();

    let output = sandbox.run(&["site", "init", "--yes"]);

    assert_eq!(code(&output), 2);
    assert_eq!(
        std::fs::read_to_string(&page).unwrap(),
        "-- my own version --",
        "a customised template must not be silently replaced"
    );
}

#[test]
fn init_uses_the_document_title_when_no_name_is_given() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new", "--title", "Hello World"]);

    sandbox.run(&["site", "init", "--yes"]);

    assert!(sandbox.root.join("hello-world").is_dir());
}

#[test]
fn init_accepts_an_explicit_name() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new", "--title", "Ignored"]);

    sandbox.run(&["site", "init", "chosen-name", "--yes"]);

    assert!(sandbox.root.join("chosen-name").is_dir());
}

#[test]
fn init_refuses_an_injection_shaped_name() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    let output = sandbox.run(&["site", "init", "../escape", "--yes"]);

    // Slugified to something harmless rather than escaping the workspace.
    assert!(!sandbox.root.parent().unwrap().join("escape").exists());
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));
}

// ---------------------------------------------------------------------------
// site publish
// ---------------------------------------------------------------------------

#[test]
fn publish_builds_and_deploys() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new", "--title", "My First Site"]);
    sandbox.run(&["site", "init", "--yes"]);

    let output = sandbox.run(&["site", "publish", "--yes"]);
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));

    let log = sandbox.invocations();
    assert!(log.contains("npm install"));
    assert!(log.contains("npm run build"));
    assert!(log.contains("wrangler pages deploy dist"));
    assert!(log.contains("--project-name my-first-site"));
}

#[test]
fn publish_reports_the_deployment_url() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);
    sandbox.run(&["site", "init", "--yes"]);

    let output = sandbox.run(&["site", "publish", "--yes"]);

    assert!(stdout(&output).contains("https://abc123.my-first-site.pages.dev"));
}

#[test]
fn publish_refuses_when_the_build_produced_nothing() {
    // §13.3 step 6. A build tool can exit zero having written nothing, and
    // deploying that would replace a working site with a blank one.
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);
    sandbox.run(&["site", "init", "--yes"]);

    // An npm that succeeds without building.
    sandbox.fake("npm", "");

    let output = sandbox.run(&["site", "publish", "--yes"]);

    assert_ne!(code(&output), 0);
    assert!(stderr(&output).to_lowercase().contains("wrote nothing"));
    assert!(
        !sandbox.invocations().contains("wrangler pages deploy"),
        "an empty build must never reach the deploy step"
    );
}

#[test]
fn publish_refuses_when_the_folder_holds_unrelated_changes() {
    // §13.3 step 3.
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);
    sandbox.run(&["site", "init", "--yes"]);

    // A git whose status reports someone else's untracked file.
    sandbox.fake(
        "git",
        r#"case "$1" in
  status) echo "?? my-private-notes.txt" ;;
  config) echo "tester@example.test" ;;
esac"#,
    );

    let output = sandbox.run(&["site", "publish", "--yes"]);

    assert_ne!(code(&output), 0);
    assert!(stdout(&output).contains("my-private-notes.txt"));
    assert!(
        !sandbox.invocations().contains("wrangler pages deploy"),
        "must stop before deploying"
    );
}

#[test]
fn publish_renders_the_current_document_every_time() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new", "--title", "Original"]);
    // Named explicitly: the folder is otherwise derived from the title, and
    // the title changes below.
    sandbox.run(&["site", "init", "my-first-site", "--yes"]);

    std::fs::write(
        sandbox.root.join("kosong.md"),
        "---\ntype: Page\ntitle: Changed\n---\n# Brand new heading\n",
    )
    .unwrap();

    sandbox.run(&["site", "publish", "--yes"]);

    let html = std::fs::read_to_string(sandbox.site_root().join("src/content/page.html")).unwrap();
    assert!(html.contains("Brand new heading"));

    let meta = std::fs::read_to_string(sandbox.site_root().join("src/content/page.json")).unwrap();
    assert!(meta.contains("Changed"));
}

#[test]
fn the_published_content_gets_the_same_safety_policy_as_preview() {
    // A regression guard. An earlier template parsed Markdown in JavaScript,
    // so raw HTML and `javascript:` URLs survived into the published page even
    // though `kosong preview` stripped them. A page must never look safe in
    // preview and publish something live.
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);
    sandbox.run(&["site", "init", "--yes"]);

    std::fs::write(
        sandbox.root.join("kosong.md"),
        "---\ntype: Page\ntitle: Probe\n---\n<script>alert(1)</script>\n\n[x](javascript:alert(1))\n",
    )
    .unwrap();

    let output = sandbox.run(&["site", "publish", "--yes"]);

    let html = std::fs::read_to_string(sandbox.site_root().join("src/content/page.html")).unwrap();
    assert!(!html.contains("<script"), "raw HTML leaked: {html}");
    assert!(
        !html.contains("javascript:"),
        "javascript URL leaked: {html}"
    );

    // And the user is told, rather than having their page silently altered.
    assert!(stderr(&output).contains("raw HTML was not shown"));
}

#[test]
fn publish_without_a_site_explains_how_to_make_one() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    let output = sandbox.run(&["site", "publish", "--yes"]);

    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("site init"));
}

// ---------------------------------------------------------------------------
// Dry run
// ---------------------------------------------------------------------------

#[test]
fn a_dry_run_publish_invokes_nothing_that_changes_anything() {
    // §12.5: "dry run does not invoke executable". The fakes log every call,
    // so the strongest proof is that no mutating tool appears in the log.
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);
    sandbox.run(&["site", "init", "--yes"]);

    std::fs::remove_file(&sandbox.log).ok();
    std::fs::remove_dir_all(sandbox.site_root().join("dist")).ok();

    let output = sandbox.run(&["site", "publish", "--dry-run"]);
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));

    let log = sandbox.invocations();
    assert!(!log.contains("npm install"), "dry run ran npm install");
    assert!(!log.contains("npm run build"), "dry run ran a build");
    assert!(!log.contains("wrangler"), "dry run reached wrangler");
    assert!(!log.contains("git commit"), "dry run made a commit");

    assert!(!sandbox.site_root().join("dist").exists());
}

#[test]
fn a_dry_run_shows_the_full_plan() {
    // Local-only steps are summarised in normal use, but a dry run is exactly
    // when the user asked to see everything.
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);
    sandbox.run(&["site", "init", "--yes"]);

    let output = sandbox.run(&["site", "publish", "--dry-run"]);
    let text = stdout(&output);

    assert!(text.contains("run "), "the exact command must be shown");
    assert!(text.contains("in "), "the working directory must be shown");
    assert!(text.contains("dry run"));
}

#[test]
fn a_dry_run_init_creates_no_repository() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    let output = sandbox.run(&["site", "init", "--dry-run"]);
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));

    assert!(
        !sandbox.invocations().contains("git init"),
        "dry run started a repository"
    );
}

// ---------------------------------------------------------------------------
// Rollback
// ---------------------------------------------------------------------------

#[test]
fn rollback_lists_history_and_is_honest_about_the_limit() {
    // Wrangler has no Pages rollback command, and the REST API would need a
    // token kosong is designed never to hold. So this lists deployments and
    // says plainly where to go, rather than implying a capability.
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);
    sandbox.run(&["site", "init", "--yes"]);

    let output = sandbox.run(&["site", "rollback"]);
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));

    assert!(
        sandbox
            .invocations()
            .contains("wrangler pages deployment list"),
        "the real deployment history must be shown"
    );

    let text = stdout(&output);
    assert!(text.contains("dash.cloudflare.com"));
    assert!(text.to_lowercase().contains("cannot"));

    // It must not delete anything in the name of rolling back.
    assert!(!sandbox.invocations().contains("deployment delete"));
}

#[test]
fn rollback_without_a_site_explains_how_to_make_one() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    let output = sandbox.run(&["site", "rollback"]);

    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("site init"));
}

// ---------------------------------------------------------------------------
// Ownership
// ---------------------------------------------------------------------------

#[test]
fn the_generated_folder_is_usable_without_kosong() {
    // The portability promise in §11: a user can move the site folder and its
    // git history and keep working without kosong installed.
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);
    sandbox.run(&["site", "init", "--yes"]);

    let package = std::fs::read_to_string(sandbox.site_root().join("package.json")).unwrap();
    assert!(package.contains("\"build\": \"astro build\""));

    // Nothing in the template refers back to kosong as a dependency.
    assert!(!package.contains("kosong-cli"));

    let config = std::fs::read_to_string(sandbox.site_root().join("astro.config.mjs")).unwrap();
    assert!(config.contains("defineConfig"));

    // kosong's own state is not part of the website.
    let ignore = std::fs::read_to_string(sandbox.site_root().join(".gitignore")).unwrap();
    assert!(ignore.contains(".kosong/"));
    assert!(Path::new(&sandbox.site_root().join(".kosong/site.toml")).exists());
}
