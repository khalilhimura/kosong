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

// The cell separator wrangler draws its table with, `│` (U+2502), is written
// as the octal escapes `\342\224\202` rather than as the character or as
// `\xe2\x94\x82`. These fakes run under `/bin/sh`, which is dash on Debian and
// Ubuntu, and dash's POSIX `printf` understands `\ooo` but not the `\xNN` form
// that bash and macOS accept. The hex form was silently emitting the literal
// text `\xe2\x94\x82` on Linux, leaving the table with no separators at all —
// so `project_exists` found nothing, a create ran where none was needed, and
// only `publish_does_not_create_a_project_that_already_exists` noticed, on
// Linux alone. Keep them octal.

/// A git that behaves enough for the workflow, and refuses the same pathspecs
/// the real one refuses.
///
/// `add` and `commit` are not permissive stubs on purpose. Real `git add` exits
/// 128 on a pathspec matching nothing, and real `git commit --only` exits 1 on
/// a path it does not already know — so a fake that shrugged at both would let
/// `site init` stage `package-lock.json`, which npm does not write until the
/// first publish, and no test here would notice until a live run did.
///
/// The fake's rule is "the path exists on disk", which is not git's rule for
/// `--only` ("git knows about it") but coincides with it in this workflow: the
/// same filtered list is staged immediately before it is committed. The exit
/// codes and the wording are the real ones, so a failure reads the way it would
/// in a terminal.
///
/// Where the two rules part company — a tracked file deleted from the folder,
/// which git knows and the disk does not — no fake can answer, so
/// `use_real_git` hands that one test the machine's own git instead.
///
/// `status` reports the lockfile as untracked whenever it is there. That is the
/// state a second publish actually finds — the fake commits nothing, so the
/// file stays untracked forever, which is the harshest version of the case and
/// the one that was failing live.
const GIT: &str = r#"case "$1" in
  status) [ -f package-lock.json ] && echo "?? package-lock.json"; exit 0 ;;
  config) echo "tester@example.test" ;;
  rev-parse) echo "main" ;;
  add)
    seen=0
    for p in "$@"; do
      if [ "$seen" = 1 ]; then
        [ -e "$p" ] || { echo "fatal: pathspec '$p' did not match any files" >&2; exit 128; }
      elif [ "$p" = "--" ]; then
        seen=1
      fi
    done
    ;;
  commit)
    seen=0
    for p in "$@"; do
      if [ "$seen" = 1 ]; then
        [ -e "$p" ] || { echo "error: pathspec '$p' did not match any file(s) known to git" >&2; exit 1; }
      elif [ "$p" = "--" ]; then
        seen=1
      fi
    done
    ;;
esac"#;

/// A git whose push cannot authenticate, exactly as one with no credential
/// helper cannot.
///
/// The message is captured from a real `git push` to an https remote with
/// helpers disabled and stdin closed — which is how kosong spawns git, and the
/// state both a GitHub Actions runner and any user who declined `gh auth
/// login`'s git question are in. `gh` is unaffected: it supplies credentials to
/// the git commands it runs itself, which is why `gh repo create --push`
/// succeeds moments before this fails.
const GIT_WHOSE_PUSH_CANNOT_AUTHENTICATE: &str = r#"case "$1" in
  status) exit 0 ;;
  config) echo "tester@example.test" ;;
  rev-parse) echo "main" ;;
  push)
    echo "fatal: could not read Username for 'https://github.com': No such device or address" >&2
    exit 128
    ;;
esac"#;

/// An npm that leaves behind what the real one leaves behind.
///
/// `install` writing `package-lock.json` is not decoration: it is the whole
/// reason the second publish used to fail, and a fake that skipped it would
/// make `a_second_publish_is_not_refused_over_npms_lockfile` pass no matter
/// what `owned_paths()` said.
const NPM: &str = r#"if [ "$1" = "install" ]; then
  printf '{ "lockfileVersion": 3 }\n' > package-lock.json
elif [ "$1" = "run" ] && [ "$2" = "build" ]; then
  mkdir -p dist && echo '<!doctype html><title>fake</title>' > dist/index.html
fi"#;

/// A wrangler whose account has no projects at all.
const WRANGLER_WITHOUT_THE_PROJECT: &str = r#"if [ "$2" = "project" ] && [ "$3" = "list" ]; then
  printf '\342\224\202 Project Name \342\224\202 Project Domains \342\224\202\n'
elif [ "$2" = "deploy" ]; then
  echo "Deployment complete! Take a peek over at https://abc123.my-first-site.pages.dev"
fi"#;

/// A wrangler whose account already holds `my-first-site`.
const WRANGLER_WITH_THE_PROJECT: &str = r#"if [ "$2" = "project" ] && [ "$3" = "list" ]; then
  printf '\342\224\202 Project Name \342\224\202 Project Domains \342\224\202\n'
  printf '\342\224\202 my-first-site \342\224\202 my-first-site.pages.dev \342\224\202\n'
elif [ "$2" = "deploy" ]; then
  echo "Deployment complete! Take a peek over at https://abc123.my-first-site.pages.dev"
fi"#;

/// A wrangler whose project list fails while everything else would work.
///
/// Deliberately not a wrangler that fails everything. That version let
/// `publish_stops_when_the_project_list_cannot_be_read` pass for the wrong
/// reason: the deploy's own failure satisfied the exit code and the stderr
/// assertion even when a failed list was being ignored completely. Failing
/// only the list means every way of mishandling it — ignoring it, reading it
/// as absence, reading it as presence — ends in a publish that succeeds, so
/// each assertion in that test has something real to catch.
const WRANGLER_WHOSE_LIST_ALONE_FAILS: &str = r#"if [ "$2" = "project" ] && [ "$3" = "list" ]; then
  echo "Authentication error [code: 10000]" >&2
  exit 1
elif [ "$2" = "deploy" ]; then
  echo "Deployment complete! Take a peek over at https://abc123.my-first-site.pages.dev"
fi"#;

/// A wrangler whose account holds `my-first-site` under someone else's name,
/// so the list does not show it but the create collides with it.
///
/// Contrived on purpose. Cloudflare project names are unique per account, and
/// the state this reproduces — absent from the list, present on the create —
/// is what a race or a stale list actually looks like from kosong's side.
const WRANGLER_WHOSE_CREATE_COLLIDES: &str = r#"if [ "$2" = "project" ] && [ "$3" = "list" ]; then
  printf '\342\224\202 Project Name \342\224\202 Project Domains \342\224\202\n'
elif [ "$2" = "project" ] && [ "$3" = "create" ]; then
  echo "A project with this name already exists" >&2
  exit 1
elif [ "$2" = "deploy" ]; then
  echo "Deployment complete! Take a peek over at https://abc123.my-first-site.pages.dev"
fi"#;

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

        // Only `use_real_git` reads this, but writing it for every sandbox
        // keeps the two kinds of test identical apart from what is on PATH.
        // The fakes ignore it, and a real git run without it would answer
        // `git config --get user.email` from whatever the machine happens to
        // have — which on a CI runner is nothing, so `site init` would stop at
        // the identity warning and commit nothing at all.
        std::fs::write(
            root.join("gitconfig"),
            "[user]\n\tname = kosong test\n\temail = tester@example.test\n\
             [commit]\n\tgpgsign = false\n",
        )
        .expect("write a git identity");

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
        self.fake("git", GIT);
        self.fake("gh", r#"[ "$1" = "auth" ] && exit 0"#);
        self.fake("npm", NPM);
        // Answers a project list with an *empty* table, so the default case in
        // these tests is a first-time publish: the project does not exist yet.
        self.fake("wrangler", WRANGLER_WITHOUT_THE_PROJECT);
    }

    /// Removes the fake git, so kosong finds the machine's own.
    ///
    /// The fake's rule is "the path exists on disk", which is the very thing at
    /// issue where a *tracked* file has been deleted: no fake can say whether
    /// git's index still holds a path without becoming a reimplementation of
    /// the index, and a reimplementation would only ever confirm its own
    /// beliefs. So one test pays for a real repository, and keeps the fake npm,
    /// gh, and wrangler — nothing there needs an account.
    ///
    /// `gh` is replaced rather than removed: removing it would let a real `gh`
    /// on the machine answer instead, and this test has no business asking
    /// anyone's GitHub anything. Failing every invocation reads as "not signed
    /// in", which kosong skips over.
    fn use_real_git(&self) {
        std::fs::remove_file(self.bin.join("git")).expect("remove the fake git");
        self.fake("gh", "exit 1");

        let found = Command::new("git").arg("--version").output();
        assert!(
            found.is_ok_and(|output| output.status.success()),
            "this test needs a real `git` on PATH"
        );
    }

    /// Asks real git something about the site folder.
    fn real_git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(self.site_root())
            .args(args)
            .env("GIT_CONFIG_GLOBAL", self.root.join("gitconfig"))
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("run git");

        assert!(
            output.status.success(),
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(binary())
            .arg("--workspace")
            .arg(&self.root)
            .args(args)
            .env("KOSONG_CONFIG_DIR", &self.config)
            .env("KOSONG_SESSION_FILE", self.config.join("session"))
            // Where a real git is used, it reads this sandbox's identity and
            // nothing of the machine's. Set for every run: the fakes ignore it,
            // and a `git` that is sometimes configured and sometimes not is a
            // difference nobody would think to look for.
            .env("GIT_CONFIG_GLOBAL", self.root.join("gitconfig"))
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
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
fn init_does_not_stage_a_lockfile_npm_has_not_written_yet() {
    // `package-lock.json` is owned, but `site init` runs before any
    // `npm install`, so it is not there. Handing it to git anyway breaks the
    // first commit twice over: `git add` exits 128 on a pathspec matching
    // nothing, and `git commit --only` exits 1 on a path git does not know.
    //
    // This is the half of the fix that has nothing to do with publishing, and
    // the half a change to `owned_paths()` alone would silently break.
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    let output = sandbox.run(&["site", "init", "--yes"]);
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));

    assert!(
        !sandbox.site_root().join("package-lock.json").exists(),
        "nothing in `site init` should have created a lockfile"
    );

    let log = sandbox.invocations();
    for line in log.lines().filter(|l| l.starts_with("git add")) {
        assert!(
            !line.contains("package-lock.json"),
            "staged a file that does not exist: {line}"
        );
    }
    for line in log.lines().filter(|l| l.starts_with("git commit")) {
        assert!(
            !line.contains("package-lock.json"),
            "committed a path git cannot know: {line}"
        );
    }
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
fn a_second_publish_is_not_refused_over_npms_lockfile() {
    // Live smoke run 30335517886, found by reading a log rather than trusting a
    // checkmark. `npm install` runs at publish step 4 and writes
    // `package-lock.json`. The lockfile was in neither `owned_paths()` nor the
    // template's `.gitignore`, so the *next* publish saw a file kosong had not
    // written, called it unrelated, and refused — telling the user to commit a
    // file kosong itself had caused to appear.
    //
    // The beginner path is `init` → `publish` ✓ → `edit` → `publish` ✗, and
    // nothing here published twice, so nothing here noticed.
    let sandbox = Sandbox::new();
    sandbox.run(&["new", "--title", "My First Site"]);
    sandbox.run(&["site", "init", "--yes"]);

    let first = sandbox.run(&["site", "publish", "--yes"]);
    assert_eq!(code(&first), 0, "stderr: {}", stderr(&first));

    // Without this the test proves nothing at all: the refusal needs a real
    // lockfile to trip over.
    assert!(
        sandbox.site_root().join("package-lock.json").is_file(),
        "the npm fake must leave a lockfile behind, as the real one does"
    );

    let second = sandbox.run(&["site", "publish", "--yes"]);
    assert_eq!(
        code(&second),
        0,
        "the second publish was refused.\nstdout: {}\nstderr: {}",
        stdout(&second),
        stderr(&second)
    );
    assert!(
        !stdout(&second).contains("kosong did not make"),
        "the lockfile was read as someone else's file: {}",
        stdout(&second)
    );

    // And it is adopted, not merely tolerated — that is what heals a site made
    // before the lockfile was owned, with no migration.
    let staged = sandbox
        .invocations()
        .lines()
        .filter(|line| line.starts_with("git add"))
        .any(|line| line.contains("package-lock.json"));
    assert!(staged, "the lockfile must be committed, not just ignored");
}

#[test]
fn publish_records_a_template_file_the_user_deleted() {
    // The gap that filtering owned paths on existence alone opened: a file the
    // user deleted is absent from the folder and still in git's index, and
    // filtering it left the deletion unstaged. The publish exited 0 while git
    // went on holding a file the folder no longer had — where the plain
    // `git add` that came before had recorded the deletion.
    //
    // `astro.config.mjs` is the one owned path that reaches this far: Astro
    // builds without it, so nothing earlier in the publish refuses. Deleting
    // `package.json` fails `npm install` instead, and deleting
    // `src/pages/index.astro` leaves an empty build that step 5 stops.
    //
    // Run against a real repository, because the claim is about git's index
    // and nothing else can answer for it.
    let sandbox = Sandbox::new();
    sandbox.use_real_git();

    sandbox.run(&["new", "--title", "My First Site"]);
    let init = sandbox.run(&["site", "init", "--yes"]);
    assert_eq!(code(&init), 0, "stderr: {}", stderr(&init));

    // Without a first commit there is nothing tracked to delete, and the test
    // would pass on an empty repository no matter what the code did.
    let tracked = sandbox.real_git(&["ls-files"]);
    assert!(
        tracked.lines().any(|path| path == "astro.config.mjs"),
        "`site init` must have committed the template first:\n{tracked}"
    );

    let first = sandbox.run(&["site", "publish", "--yes"]);
    assert_eq!(code(&first), 0, "stderr: {}", stderr(&first));

    std::fs::remove_file(sandbox.site_root().join("astro.config.mjs"))
        .expect("delete a template file");

    let second = sandbox.run(&["site", "publish", "--yes"]);
    assert_eq!(
        code(&second),
        0,
        "the publish stopped.\nstdout: {}\nstderr: {}",
        stdout(&second),
        stderr(&second)
    );

    let tracked = sandbox.real_git(&["ls-files"]);
    assert!(
        !tracked.lines().any(|path| path == "astro.config.mjs"),
        "git still tracks a file the folder no longer has:\n{tracked}"
    );

    let status = sandbox.real_git(&["status", "--porcelain"]);
    assert!(
        !status.contains("astro.config.mjs"),
        "the deletion was left unstaged:\n{status}"
    );
}

#[test]
fn a_push_that_cannot_authenticate_says_why_and_how_to_fix_it() {
    // Seen in every live smoke run so far, and reported only as
    // `! could not send to GitHub; your page will still be published` — a
    // sentence with no cause and no next action. git's own reason was read and
    // thrown away, so no run could say whether the remote was wrong, the branch
    // was behind, or, as it turned out, nothing had told git who the user was.
    //
    // Not CI-only, which is why this is a product test and not a workflow fix
    // alone: `gh auth login` asks whether to set git up too, and answering no —
    // or authenticating by `GH_TOKEN` alone — leaves `gh auth status` passing,
    // so kosong offers GitHub and records the repository, and then every push
    // fails exactly like this.
    let sandbox = Sandbox::new();
    sandbox.run(&["new", "--title", "My First Site"]);
    // A signed-in gh, so a repository is recorded and the push is attempted.
    sandbox.run(&["site", "init", "--yes"]);
    sandbox.fake("git", GIT_WHOSE_PUSH_CANNOT_AUTHENTICATE);

    let output = sandbox.run(&["site", "publish", "--yes"]);

    // The deployment is what the user asked for; a failed push must not sink
    // it. That part was already right and must stay right.
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));
    assert!(
        sandbox.invocations().contains("wrangler pages deploy"),
        "the publish must still have deployed"
    );

    // Asserted on both streams together: what matters is that it reaches the
    // user, not which pipe carries it.
    let told = format!("{}{}", stdout(&output), stderr(&output));

    assert!(
        told.contains("could not read Username"),
        "git's own reason must survive: {told}"
    );
    assert!(
        told.contains("gh auth setup-git"),
        "the repair must name the command that fixes it: {told}"
    );
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

#[test]
fn publish_creates_the_project_when_it_does_not_exist_yet() {
    // The bug this fixes: `wrangler pages deploy` fails outright when the
    // project is absent, naming a command kosong did not offer. A first-time
    // publish could not succeed.
    let sandbox = Sandbox::new();
    sandbox.run(&["new", "--title", "My First Site"]);
    sandbox.run(&["site", "init", "--yes"]);

    let output = sandbox.run(&["site", "publish", "--yes"]);
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));

    let log = sandbox.invocations();
    assert!(log.contains("wrangler pages project list"));
    assert!(
        log.contains("wrangler pages project create my-first-site --production-branch main"),
        "exact argv, including the branch: log was\n{log}"
    );

    // Order matters: a deploy before the create is the failure we are fixing.
    let created = log.find("pages project create").expect("a create");
    let deployed = log.find("pages deploy").expect("a deploy");
    assert!(
        created < deployed,
        "the project must exist before the deploy"
    );
}

#[test]
fn publish_does_not_create_a_project_that_already_exists() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new", "--title", "My First Site"]);
    sandbox.run(&["site", "init", "--yes"]);
    sandbox.fake("wrangler", WRANGLER_WITH_THE_PROJECT);

    let output = sandbox.run(&["site", "publish", "--yes"]);
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));

    let log = sandbox.invocations();
    assert!(log.contains("wrangler pages project list"));
    assert!(
        !log.contains("pages project create"),
        "the project existed; creating it again would be a needless prompt"
    );
    assert!(log.contains("wrangler pages deploy dist"));
}

#[test]
fn publish_stops_when_the_project_list_cannot_be_read() {
    // A failed list is not an absent project. Treating it as absent would
    // attempt a create that fails for the same underlying reason, and report
    // the wrong cause: "could not create" when the truth is "you are signed
    // out".
    let sandbox = Sandbox::new();
    sandbox.run(&["new", "--title", "My First Site"]);
    sandbox.run(&["site", "init", "--yes"]);
    // Only the list fails. Everything after it would succeed, so a publish
    // that reaches the end is proof the failure was swallowed.
    sandbox.fake("wrangler", WRANGLER_WHOSE_LIST_ALONE_FAILS);

    let output = sandbox.run(&["site", "publish", "--yes"]);

    assert_ne!(
        code(&output),
        0,
        "the publish ran to completion despite not knowing whether the project exists"
    );
    assert!(
        stderr(&output).contains("Authentication error"),
        "the provider's own error must survive: {}",
        stderr(&output)
    );

    let log = sandbox.invocations();
    assert!(!log.contains("pages project create"), "must not guess");
    assert!(!log.contains("pages deploy"), "must not deploy blind");
}

#[test]
fn a_taken_project_name_stops_the_publish_and_says_where_to_change_it() {
    // Pages project names are unique per account. Deploying anyway would put
    // this user's files into a project that is not theirs, so a collision has
    // to stop the publish rather than be treated as "the project exists, carry
    // on" — which is the tempting reading, and the wrong one.
    let sandbox = Sandbox::new();
    sandbox.run(&["new", "--title", "My First Site"]);
    sandbox.run(&["site", "init", "--yes"]);
    sandbox.fake("wrangler", WRANGLER_WHOSE_CREATE_COLLIDES);

    let output = sandbox.run(&["site", "publish", "--yes"]);

    assert_ne!(
        code(&output),
        0,
        "a collision must not be a successful publish"
    );

    let complaint = stderr(&output);
    assert!(
        complaint.contains("my-first-site"),
        "the error must name the taken name: {complaint}"
    );
    assert!(
        complaint.contains("site.toml") || complaint.contains("site init"),
        "the repair must say where to change it: {complaint}"
    );

    let log = sandbox.invocations();
    assert!(
        log.contains("pages project create"),
        "the create must have been attempted: {log}"
    );
    assert!(
        !log.contains("pages deploy"),
        "a taken name must never reach the deploy: {log}"
    );
}

#[test]
fn publish_asks_before_creating_a_project_and_stops_when_it_is_not_answered() {
    // The create is disclosed and confirmed on its own rather than folded into
    // the deploy's approval. There is no terminal here, so `Ui::confirm` takes
    // its default of no — which makes this the test for that separation: the
    // read-only list runs, and then nothing else does. If the create were ever
    // folded into the deploy's single approval, one of the two would slip
    // through this gate.
    //
    // The site is set up with a gh that is not signed in, so no GitHub remote
    // is recorded. That matters: with one, the push would ask first and stop
    // the publish before Cloudflare was reached at all, and this test would
    // pass while proving nothing about the create.
    let sandbox = Sandbox::new();
    sandbox.run(&["new", "--title", "My First Site"]);
    sandbox.fake("gh", "exit 1");
    sandbox.run(&["site", "init", "--yes"]);

    let output = sandbox.run(&["site", "publish"]);

    assert_ne!(code(&output), 0, "an unanswered prompt is not a publish");

    let log = sandbox.invocations();
    assert!(
        log.contains("pages project list"),
        "the read-only check may run without asking: {log}"
    );
    assert!(
        !log.contains("pages project create"),
        "nothing may be created without an answer: {log}"
    );
    assert!(
        !log.contains("pages deploy"),
        "nothing may be deployed without an answer: {log}"
    );
}

#[test]
fn a_dry_run_that_cannot_read_the_project_list_fails_rather_than_guessing() {
    // A dry run gained a way to fail that it did not have before: the list is
    // read-only, so it runs even here, and someone signed out gets wrangler's
    // error where they used to get a plan. That is deliberate. A plan that
    // cannot tell whether a create belongs in it would be a guess printed as a
    // fact, which is the same wrong answer as treating a failed list as an
    // absent project.
    let sandbox = Sandbox::new();
    sandbox.run(&["new", "--title", "My First Site"]);
    sandbox.run(&["site", "init", "--yes"]);
    sandbox.fake("wrangler", WRANGLER_WHOSE_LIST_ALONE_FAILS);

    let output = sandbox.run(&["site", "publish", "--dry-run"]);

    assert_ne!(
        code(&output),
        0,
        "a plan that cannot be known must not be printed"
    );
    assert!(
        !sandbox.invocations().contains("pages deploy"),
        "a dry run must never deploy, least of all a failing one"
    );
}

#[test]
fn a_dry_run_names_both_the_project_and_the_deploy() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new", "--title", "My First Site"]);
    sandbox.run(&["site", "init", "--yes"]);

    let output = sandbox.run(&["site", "publish", "--dry-run"]);
    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));

    let text = stdout(&output);
    assert!(
        text.contains("pages project create"),
        "the plan must name the create: {text}"
    );
    assert!(
        text.contains("pages deploy"),
        "showing the create must not swallow the deploy: {text}"
    );
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
    // §7: read-only checks may run under a dry run — they are how the plan is
    // built. What must never run is a mutating one.
    assert!(
        !log.contains("wrangler pages deploy"),
        "dry run reached a deploy"
    );
    assert!(
        !log.contains("wrangler pages project create"),
        "dry run created a project"
    );
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
