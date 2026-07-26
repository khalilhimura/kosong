//! End-to-end tests driving the real binary.
//!
//! These assert the things a user actually experiences: exit codes, what
//! appears on stdout versus stderr, and whether an error tells you what to do
//! next. Unit tests cannot catch a command that reports success while writing
//! nothing, so the happy path is exercised through the built binary.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

/// Path to the binary under test, as built by cargo for this test run.
fn binary() -> PathBuf {
    // target/debug/deps/cli-<hash> → target/debug/kosong
    let mut path = std::env::current_exe().expect("test binary path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("kosong")
}

/// A workspace plus an isolated config directory.
struct Sandbox {
    _guard: TempDir,
    root: PathBuf,
    config: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let guard = TempDir::new().expect("temp dir");
        let root = std::fs::canonicalize(guard.path()).expect("canonicalize");
        let config = root.join("config");
        std::fs::create_dir_all(&config).expect("config dir");
        Self {
            _guard: guard,
            root,
            config,
        }
    }

    /// Runs kosong with this sandbox's workspace and config.
    ///
    /// `NO_COLOR` and a non-terminal stdin keep output deterministic and stop
    /// any prompt from blocking.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(binary())
            .arg("--workspace")
            .arg(&self.root)
            .args(args)
            .env("KOSONG_CONFIG_DIR", &self.config)
            // Never the developer's real keychain.
            .env("KOSONG_SESSION_FILE", self.session_file())
            // An unroutable port, so a test that accidentally reaches the
            // network fails fast rather than hitting the real service.
            .env("KOSONG_API_URL", "http://127.0.0.1:9")
            .env("NO_COLOR", "1")
            .env_remove("EDITOR")
            .env_remove("VISUAL")
            .output()
            .expect("run kosong")
    }

    fn session_file(&self) -> PathBuf {
        self.config.join("session")
    }

    /// Plants a stored refresh token, so a test can act as if signed in.
    fn write_session(&self, token: &str) {
        std::fs::write(self.session_file(), token).expect("write session");
    }

    fn document(&self) -> PathBuf {
        self.root.join("kosong.md")
    }

    fn write_document(&self, contents: &str) {
        std::fs::write(self.document(), contents).expect("write document");
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("process exited normally")
}

// ---------------------------------------------------------------------------
// new
// ---------------------------------------------------------------------------

#[test]
fn new_creates_a_conformant_document() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["new", "--title", "My First Site"]);

    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));
    assert!(sandbox.document().is_file(), "no file was written");

    let contents = std::fs::read_to_string(sandbox.document()).unwrap();
    assert!(contents.starts_with("---\n"));
    assert!(contents.contains("type: Page"));
    assert!(contents.contains("title: My First Site"));
    assert!(contents.contains("profile: 1"));
    assert!(contents.contains("slug: my-first-site"));
    assert!(contents.contains("# My First Site"));
}

#[test]
fn new_refuses_to_overwrite_an_existing_document() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new", "--title", "First"]);

    let output = sandbox.run(&["new", "--title", "Second"]);

    assert_eq!(code(&output), 2);
    let contents = std::fs::read_to_string(sandbox.document()).unwrap();
    assert!(
        contents.contains("title: First"),
        "the original document must survive"
    );
}

#[test]
fn new_tells_the_user_what_to_do_next() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["new"]);

    assert!(stdout(&output).contains("Next: kosong edit"));
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

#[test]
fn status_on_an_empty_folder_is_not_an_error() {
    // Having made nothing yet is a legitimate state, not a failure.
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["status"]);

    assert_eq!(code(&output), 0);
    assert!(stdout(&output).contains("no page yet"));
    assert!(stdout(&output).contains("kosong start"));
}

#[test]
fn status_reports_a_healthy_document() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new", "--title", "My First Site"]);

    let output = sandbox.run(&["status"]);

    assert_eq!(code(&output), 0);
    assert!(stdout(&output).contains("My First Site"));
    assert!(stdout(&output).contains("my-first-site"));
}

#[test]
fn status_json_is_parseable_and_stable() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new", "--title", "My First Site"]);

    let output = sandbox.run(&["status", "--json"]);
    assert_eq!(code(&output), 0);

    let json: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("status --json must emit valid JSON");

    assert_eq!(json["schema"], 1);
    assert_eq!(json["okf_version"], "0.1");
    assert_eq!(json["document"]["valid"], true);
    assert_eq!(json["document"]["managed"], true);
    assert_eq!(json["document"]["type"], "Page");
    assert_eq!(json["document"]["slug"], "my-first-site");
    assert_eq!(json["onboarding"]["local_document_created"], true);
    assert_eq!(json["session"]["signed_in"], false);
}

#[test]
fn status_json_never_leaks_a_secret() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    let text = stdout(&sandbox.run(&["status", "--json"])).to_lowercase();

    for forbidden in ["token", "password", "secret", "refresh", "@", "bearer"] {
        assert!(
            !text.contains(forbidden),
            "status --json must not contain `{forbidden}`"
        );
    }
}

#[test]
fn status_reports_an_invalid_document_and_exits_two() {
    let sandbox = Sandbox::new();
    sandbox.write_document("---\ntitle: no type field\n---\nbody\n");

    let output = sandbox.run(&["status"]);

    assert_eq!(code(&output), 2);
    assert!(stdout(&output).contains("needs fixing"));
    // The repair action goes to stderr with the error.
    assert!(stderr(&output).contains("kosong doctor"));
}

#[test]
fn status_json_still_emits_json_when_the_document_is_broken() {
    // A script must be able to read the reason, not just see a non-zero exit.
    let sandbox = Sandbox::new();
    sandbox.write_document("---\ntitle: no type\n---\nbody\n");

    let output = sandbox.run(&["status", "--json"]);

    assert_eq!(code(&output), 2);
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("valid JSON even on failure");
    assert_eq!(json["document"]["exists"], true);
    assert_eq!(json["document"]["valid"], false);
    assert!(json["document"]["error"].is_string());
}

#[test]
fn status_recognises_an_unmanaged_okf_document() {
    // A document from another OKF producer is readable, not an error.
    let sandbox = Sandbox::new();
    sandbox.write_document("---\ntype: BigQuery Table\ntitle: Orders\n---\n# Schema\n");

    let output = sandbox.run(&["status", "--json"]);

    assert_eq!(code(&output), 0);
    let json: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(json["document"]["valid"], true);
    assert_eq!(json["document"]["managed"], false);
    assert_eq!(json["document"]["type"], "BigQuery Table");
}

// ---------------------------------------------------------------------------
// doctor
// ---------------------------------------------------------------------------

#[test]
fn doctor_json_is_parseable() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    let output = sandbox.run(&["doctor", "--json"]);

    let json: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("doctor --json must emit valid JSON");
    assert_eq!(json["schema"], 1);
    assert!(json["checks"].is_array());
    assert!(!json["checks"].as_array().unwrap().is_empty());
}

#[test]
fn every_failing_doctor_check_carries_a_repair() {
    // The Phase A exit gate: every diagnostic failure has an actionable next
    // step. A check that reports a problem with no way forward is the exact
    // experience kosong exists to replace.
    let sandbox = Sandbox::new();
    sandbox.write_document("---\ntitle: broken\n---\n");

    let output = sandbox.run(&["doctor", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();

    for check in json["checks"].as_array().unwrap() {
        if check["status"] != "ok" {
            let repair = check["repair"].as_str().unwrap_or("");
            assert!(
                !repair.trim().is_empty(),
                "check `{}` is not ok but offers no repair",
                check["name"]
            );
        }
    }
}

#[test]
fn doctor_does_not_fail_merely_because_optional_tools_are_missing() {
    // Local-first: a machine with no gh, wrangler, or npm can still work.
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    let output = Command::new(binary())
        .arg("--workspace")
        .arg(&sandbox.root)
        .args(["doctor", "--json"])
        .env("KOSONG_CONFIG_DIR", &sandbox.config)
        .env("NO_COLOR", "1")
        // An empty PATH means no external tool can be found at all.
        .env("PATH", "")
        .output()
        .expect("run kosong");

    let json: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(
        json["healthy"], true,
        "missing optional tools must not block local work"
    );
    assert_eq!(code(&output), 0);
}

// ---------------------------------------------------------------------------
// show
// ---------------------------------------------------------------------------

#[test]
fn show_without_a_document_explains_how_to_start() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["show"]);

    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("kosong start"));
}

#[test]
fn show_raw_prints_the_file_byte_for_byte() {
    let sandbox = Sandbox::new();
    let source = "---\ntype: Page\ntitle: Exact\nunknown_key: preserved\n---\n# Body\n";
    sandbox.write_document(source);

    let output = sandbox.run(&["show", "--raw"]);

    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output).trim_end(), source.trim_end());
    assert!(stdout(&output).contains("unknown_key: preserved"));
}

// ---------------------------------------------------------------------------
// edit
// ---------------------------------------------------------------------------

#[test]
fn edit_refuses_a_shell_command_as_an_editor() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    let output = Command::new(binary())
        .arg("--workspace")
        .arg(&sandbox.root)
        .args(["edit"])
        .env("KOSONG_CONFIG_DIR", &sandbox.config)
        .env("NO_COLOR", "1")
        .env("EDITOR", "vim; rm -rf ~")
        .output()
        .expect("run kosong");

    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("shell"));
    // Nothing was deleted and nothing was launched.
    assert!(sandbox.document().is_file());
}

#[test]
fn edit_dry_run_shows_the_command_without_running_it() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    let output = Command::new(binary())
        .arg("--workspace")
        .arg(&sandbox.root)
        .args(["edit", "--dry-run"])
        .env("KOSONG_CONFIG_DIR", &sandbox.config)
        .env("NO_COLOR", "1")
        .env("EDITOR", "definitely-not-a-real-editor --wait")
        .output()
        .expect("run kosong");

    assert_eq!(code(&output), 0, "dry run must not try to launch");
    assert!(stdout(&output).contains("definitely-not-a-real-editor --wait"));
    assert!(stdout(&output).contains("kosong.md"));
}

#[test]
fn edit_without_an_editor_says_how_to_set_one() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    let output = sandbox.run(&["edit"]);

    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("EDITOR"));
}

// ---------------------------------------------------------------------------
// Global behaviour
// ---------------------------------------------------------------------------

#[test]
fn quiet_suppresses_prose_but_not_errors() {
    let sandbox = Sandbox::new();

    let created = sandbox.run(&["--quiet", "new"]);
    assert_eq!(code(&created), 0);
    assert!(
        !stdout(&created).contains("Next:"),
        "--quiet must hide the lesson and next command"
    );

    let failed = sandbox.run(&["--quiet", "new"]);
    assert_eq!(code(&failed), 2);
    assert!(
        !stderr(&failed).is_empty(),
        "--quiet must never hide an error"
    );
}

#[test]
fn no_color_output_contains_no_escape_sequences() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    let output = sandbox.run(&["status"]);

    assert!(
        !stdout(&output).contains('\x1b'),
        "NO_COLOR output must contain no ANSI escapes"
    );
}

#[test]
fn status_marks_carry_meaning_without_colour() {
    // Accessibility: never colour-only meaning.
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    let output = sandbox.run(&["status"]);
    let text = stdout(&output);

    assert!(
        text.contains('✔') || text.contains('·'),
        "status lines must carry a symbol: {text}"
    );
}

#[test]
fn help_lists_only_commands_that_work() {
    let output = Command::new(binary())
        .arg("--help")
        .env("NO_COLOR", "1")
        .output()
        .expect("run kosong --help");

    let text = stdout(&output);
    assert_eq!(code(&output), 0);

    for implemented in [
        "start",
        "new",
        "edit",
        "show",
        "preview",
        "status",
        "doctor",
        "login",
        "logout",
        "delete-account",
        "sync",
        "site",
        "gh",
        "cf",
    ] {
        assert!(
            text.contains(implemented),
            "`{implemented}` missing from help"
        );
    }
    // Advertising a command that is not built yet teaches the wrong lesson
    // about whether the tool can be trusted. `update --check` is the last
    // command in §5.1 still to be built.
    assert!(
        !text.contains("update"),
        "`update` is advertised but not implemented"
    );
}

#[test]
fn an_unknown_command_fails_without_panicking() {
    let output = Command::new(binary())
        .arg("definitely-not-a-command")
        .env("NO_COLOR", "1")
        .output()
        .expect("run kosong");

    assert_ne!(code(&output), 0);
    assert!(!stderr(&output).contains("panicked"));
}

#[test]
fn a_reserved_filename_is_refused() {
    let sandbox = Sandbox::new();
    let target = sandbox.root.join("index.md");

    let output = sandbox.run(&["new", "--path", target.to_str().unwrap()]);

    assert_eq!(code(&output), 2);
    assert!(!Path::new(&target).exists(), "index.md must not be created");
    assert!(
        stderr(&output)
            .to_lowercase()
            .contains("open knowledge format")
    );
}

// ---------------------------------------------------------------------------
// Sessions and sync
// ---------------------------------------------------------------------------

#[test]
fn commands_needing_an_account_say_so_when_signed_out() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    let output = sandbox.run(&["sync"]);

    // Exit 3 is the authentication code from §5.3.
    assert_eq!(code(&output), 3);
    assert!(stderr(&output).contains("kosong login"));
}

#[test]
fn signing_in_as_someone_else_is_refused_rather_than_silently_ignored() {
    // Regression: `login --email` used to report success while leaving the
    // previous session in place, so the next command ran as the *old* user.
    // Someone could believe they were working as one account while operating
    // as another.
    let sandbox = Sandbox::new();
    sandbox.write_session("a-stored-refresh-token");

    let output = sandbox.run(&[
        "login",
        "--email",
        "someone.else@example.com",
        "--code",
        "123456",
    ]);

    assert_eq!(code(&output), 2, "must not report success");
    assert!(stderr(&output).contains("already signed in"));
    assert!(stderr(&output).contains("kosong logout"));
}

#[test]
fn an_existing_session_is_reported_without_an_email_argument() {
    let sandbox = Sandbox::new();
    sandbox.write_session("a-stored-refresh-token");

    let output = sandbox.run(&["login"]);

    assert_eq!(code(&output), 0);
    assert!(stdout(&output).contains("already signed in"));
}

#[test]
fn a_malformed_email_is_caught_before_any_request() {
    // The API URL points at a dead port, so reaching the network at all would
    // surface as a connection error rather than this message.
    let sandbox = Sandbox::new();

    let output = sandbox.run(&["login", "--email", "not-an-address"]);

    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("does not look like an email"));
}

#[test]
fn logging_out_when_signed_out_is_not_an_error() {
    let sandbox = Sandbox::new();

    let output = sandbox.run(&["logout"]);

    assert_eq!(code(&output), 0);
    assert!(stdout(&output).contains("not signed in"));
}

#[test]
fn logging_out_clears_the_credential_even_offline() {
    // The user asked to be signed out. Leaving a token on disk because the
    // server was unreachable would be the wrong way to fail.
    let sandbox = Sandbox::new();
    sandbox.write_session("a-stored-refresh-token");

    let output = sandbox.run(&["logout"]);

    assert_eq!(code(&output), 0);
    assert!(
        !sandbox.session_file().exists(),
        "the stored credential must be gone"
    );
}

#[test]
fn status_reports_the_session_without_touching_the_network() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    let signed_out = sandbox.run(&["status", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&stdout(&signed_out)).unwrap();
    assert_eq!(json["session"]["signed_in"], false);

    sandbox.write_session("a-stored-refresh-token");
    let signed_in = sandbox.run(&["status", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&stdout(&signed_in)).unwrap();
    assert_eq!(json["session"]["signed_in"], true);
    // Where the credential lives is reported, so a file fallback is visible.
    assert!(json["session"]["stored_in"].is_string());
}

#[test]
fn status_still_leaks_no_secret_when_signed_in() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);
    sandbox.write_session("super-secret-refresh-token-value");

    let text = stdout(&sandbox.run(&["status", "--json"]));

    assert!(!text.contains("super-secret-refresh-token-value"));
    for forbidden in ["token", "password", "bearer", "@"] {
        assert!(
            !text.to_lowercase().contains(forbidden),
            "status --json must not contain `{forbidden}`"
        );
    }
}

#[test]
fn deleting_an_account_while_signed_out_says_how_to_sign_in() {
    let sandbox = Sandbox::new();

    let output = sandbox.run(&["delete-account", "--yes"]);

    assert_eq!(code(&output), 3, "stderr: {}", stderr(&output));
    assert!(stderr(&output).contains("kosong login"));
}

#[test]
fn deleting_an_account_dry_run_deletes_nothing() {
    // The sandbox points at a dead port, so a dry run that reached the network
    // would fail with the network code rather than succeeding.
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);
    sandbox.write_session("a-stored-refresh-token");

    let output = sandbox.run(&["delete-account", "--dry-run"]);

    assert_eq!(code(&output), 0, "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("nothing was deleted"));
    assert!(sandbox.session_file().exists(), "the sign-in must survive");
    assert!(sandbox.document().is_file(), "the page must survive");
}

#[test]
fn deleting_an_account_states_what_survives_it() {
    // The dangerous misunderstanding is that this takes a published page down.
    // It does not, and the command has to say so before it asks.
    let sandbox = Sandbox::new();
    sandbox.write_session("a-stored-refresh-token");

    let text = stdout(&sandbox.run(&["delete-account", "--dry-run"]));

    assert!(
        text.contains("kosong.md"),
        "must name the local file: {text}"
    );
    assert!(
        text.to_lowercase().contains("published"),
        "must address an already-published site: {text}"
    );
}

#[test]
fn deleting_an_account_without_a_terminal_needs_an_explicit_yes() {
    // stdin is not a terminal here, so the confirmation takes its default. That
    // default must be no: an unattended script must not be able to delete an
    // account by omission.
    let sandbox = Sandbox::new();
    sandbox.write_session("a-stored-refresh-token");

    let output = sandbox.run(&["delete-account"]);

    assert_eq!(code(&output), 2, "stderr: {}", stderr(&output));
    assert!(stderr(&output).contains("Nothing was deleted"));
    assert!(sandbox.session_file().exists(), "the sign-in must survive");
}

#[test]
fn a_failed_deletion_keeps_the_sign_in() {
    // The account is still there, so the credential for it must be too.
    // Clearing it locally would leave the user unable to try again.
    let sandbox = Sandbox::new();
    sandbox.write_session("a-stored-refresh-token");

    let output = sandbox.run(&["delete-account", "--yes"]);

    assert_eq!(code(&output), 5, "stderr: {}", stderr(&output));
    assert!(
        sandbox.session_file().exists(),
        "a network failure must not sign the user out"
    );
}

#[test]
fn push_and_pull_cannot_be_combined() {
    let sandbox = Sandbox::new();

    let output = sandbox.run(&["sync", "--push", "--pull"]);

    assert_ne!(code(&output), 0);
    assert!(!stderr(&output).contains("panicked"));
}

#[test]
fn a_network_failure_is_reported_as_one() {
    // The sandbox points at a dead port.
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);
    sandbox.write_session("a-stored-refresh-token");

    let output = sandbox.run(&["sync"]);

    // Exit 5 is the network code from §5.3.
    assert_eq!(code(&output), 5, "stderr: {}", stderr(&output));
    assert!(stderr(&output).to_lowercase().contains("connection"));
}

// ---------------------------------------------------------------------------
// Provider commands
// ---------------------------------------------------------------------------

#[test]
fn provider_commands_appear_in_help() {
    let output = Command::new(binary())
        .arg("--help")
        .env("NO_COLOR", "1")
        .output()
        .expect("run kosong --help");

    let text = stdout(&output);
    assert!(text.contains("gh"));
    assert!(text.contains("cf"));
}

#[test]
fn only_allowlisted_provider_subcommands_are_accepted() {
    // §6.2 puts arbitrary pass-through out of scope. An unlisted subcommand
    // must be rejected by argument parsing, never forwarded to the tool.
    let sandbox = Sandbox::new();

    for forbidden in ["push", "release", "secret", "api", "auth login"] {
        let output = sandbox.run(&["gh", forbidden]);
        assert_ne!(code(&output), 0, "`gh {forbidden}` must be refused");
        assert!(!stderr(&output).contains("panicked"));
    }

    for forbidden in ["deploy", "delete", "login", "rollback"] {
        let output = sandbox.run(&["cf", forbidden]);
        assert_ne!(code(&output), 0, "`cf {forbidden}` must be refused");
    }
}

#[test]
fn an_injection_shaped_repository_name_is_refused_before_anything_runs() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    for hostile in ["../../etc/passwd", "owner;rm -rf /", "name$(whoami)"] {
        let output = sandbox.run(&["gh", "repo", hostile]);

        // Exit 2 is the input code, not 4: this never reached `gh`.
        assert_eq!(code(&output), 2, "`{hostile}` must be refused as input");
        assert!(stderr(&output).contains("usable"));
    }
}

#[test]
fn a_missing_provider_tool_maps_to_the_provider_exit_code() {
    // §12.4: external tool failures map to exit code 4.
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    let output = Command::new(binary())
        .arg("--workspace")
        .arg(&sandbox.root)
        .args(["gh", "auth"])
        .env("KOSONG_CONFIG_DIR", &sandbox.config)
        .env("KOSONG_SESSION_FILE", sandbox.session_file())
        .env("NO_COLOR", "1")
        // No gh on an empty PATH.
        .env("PATH", "")
        .output()
        .expect("run kosong");

    assert_eq!(code(&output), 4, "stderr: {}", stderr(&output));
    assert!(stderr(&output).contains("not installed"));
}

#[test]
fn cf_deployments_without_a_site_explains_rather_than_failing_obscurely() {
    let sandbox = Sandbox::new();
    sandbox.run(&["new"]);

    let output = sandbox.run(&["cf", "deployments"]);

    assert_eq!(code(&output), 2);
    assert!(stderr(&output).contains("site init"));
}
