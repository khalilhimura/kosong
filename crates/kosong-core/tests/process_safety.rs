//! Process adapter safety, per technical specification §12.5.
//!
//! Required cases: arguments with spaces and Unicode, arguments containing
//! shell metacharacters, path traversal attempts, timeout and cancellation,
//! non-zero child exit mapping, dry run not invoking the executable, and
//! redaction of token-shaped output.
//!
//! The metacharacter tests use a **fake binary that prints its argv one per
//! line**. That turns "no shell is involved" from a claim into an observation:
//! if a `;` were ever interpreted, the echoed argv would differ from what was
//! passed. Asserting it in prose would prove nothing.

use camino::{Utf8Path, Utf8PathBuf};
use kosong_core::process::{
    LONG_TIMEOUT, ProcessError, SafeCommand, format_command,
};
use kosong_core::redact::{REDACTED, redact};
use kosong_core::providers::cloudflare::CloudflareOperation;
use kosong_core::providers::github::{GitHubOperation, gh_repair, interpret_auth_status};
use kosong_core::providers::{AuthState, Operation, ProviderError, validate_name};
use std::time::Duration;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Fake binaries
// ---------------------------------------------------------------------------

/// A directory of fake executables for tests to invoke.
struct FakeBins {
    _guard: TempDir,
    dir: Utf8PathBuf,
}

impl FakeBins {
    fn new() -> Self {
        let guard = TempDir::new().expect("temp dir");
        let dir =
            Utf8PathBuf::from_path_buf(std::fs::canonicalize(guard.path()).expect("canonicalize"))
                .expect("utf-8 path");
        Self { _guard: guard, dir }
    }

    /// Writes an executable script.
    ///
    /// The script has a shebang, which the kernel honours. That is not kosong
    /// invoking a shell: kosong execs this file directly, exactly as it would
    /// exec a compiled binary.
    fn write(&self, name: &str, body: &str) -> Utf8PathBuf {
        let path = self.dir.join(name);
        std::fs::write(&path, body).expect("write fake binary");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod fake binary");
        }
        path
    }

    /// A binary that prints each argument on its own line.
    fn argv_echo(&self) -> Utf8PathBuf {
        self.write("argv-echo", "#!/bin/sh\nprintf '%s\\n' \"$@\"\n")
    }

    fn path(&self) -> &Utf8Path {
        &self.dir
    }
}

// ---------------------------------------------------------------------------
// Exec'ing a file this test just wrote
// ---------------------------------------------------------------------------

/// How many times a spawn gets to hit a busy executable before we believe it.
const SPAWN_ATTEMPTS: usize = 10;

/// Long enough for a forked child to reach its own exec, short enough that a
/// genuine failure is still reported promptly.
const SPAWN_RETRY_PAUSE: Duration = Duration::from_millis(20);

/// Runs `attempt`, retrying only while it fails with `ETXTBSY`.
///
/// Writing an executable and exec'ing it immediately races with every other
/// test in this binary, because the harness runs tests on parallel threads.
/// On Linux `posix_spawn` clones a child that gets a *copy* of the descriptor
/// table, so a child forked while `FakeBins::write` still holds the file open
/// for writing keeps that descriptor until its own exec clears it — and
/// exec'ing a file some process holds open for writing is `ETXTBSY`.
///
/// The condition is transient by construction: it lasts only until that child
/// execs. So it is the harness's problem, not `kosong_core::process`'s, which
/// must go on reporting a spawn failure faithfully. Retries that run out
/// return the original error rather than swallowing it, and any other failure
/// is returned on the first attempt so that a real bug still surfaces at once.
fn with_spawn_retry<T>(
    mut attempt: impl FnMut() -> Result<T, ProcessError>,
) -> Result<T, ProcessError> {
    let mut outcome = attempt();

    for _ in 1..SPAWN_ATTEMPTS {
        match &outcome {
            Err(ProcessError::Spawn { source, .. })
                if source.kind() == std::io::ErrorKind::ExecutableFileBusy => {}
            _ => break,
        }
        std::thread::sleep(SPAWN_RETRY_PAUSE);
        outcome = attempt();
    }

    outcome
}

/// Drives a future to completion on a temporary tokio runtime.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(future)
}

/// Runs a command on a temporary tokio runtime.
fn run(command: SafeCommand) -> Result<kosong_core::process::ProcessResult, ProcessError> {
    with_spawn_retry(|| block_on(command.run()))
}

/// A spawn that failed because the executable was open for writing.
fn busy_executable() -> ProcessError {
    ProcessError::Spawn {
        program: "fake".into(),
        source: std::io::Error::from(std::io::ErrorKind::ExecutableFileBusy),
    }
}

#[test]
fn a_busy_executable_is_waited_out_rather_than_reported() {
    // The race cannot be provoked on demand, so the policy is pinned directly.
    let mut attempts = 0;

    let outcome = with_spawn_retry(|| {
        attempts += 1;
        if attempts < 3 {
            Err(busy_executable())
        } else {
            Ok("ran")
        }
    });

    assert_eq!(outcome.expect("must succeed once the file is free"), "ran");
    assert_eq!(attempts, 3);
}

#[test]
fn an_executable_that_stays_busy_is_reported_in_the_end() {
    // Retrying must not turn a real, persistent failure into silence.
    let mut attempts = 0;

    let outcome: Result<(), ProcessError> = with_spawn_retry(|| {
        attempts += 1;
        Err(busy_executable())
    });

    assert!(matches!(outcome, Err(ProcessError::Spawn { .. })));
    assert_eq!(attempts, SPAWN_ATTEMPTS);
}

#[test]
fn any_other_spawn_failure_is_reported_at_once() {
    // A missing tool is not a race, and waiting 200ms to say so helps nobody.
    let mut attempts = 0;

    let outcome: Result<(), ProcessError> = with_spawn_retry(|| {
        attempts += 1;
        Err(ProcessError::NotFound {
            program: "nope".into(),
        })
    });

    assert!(matches!(outcome, Err(ProcessError::NotFound { .. })));
    assert_eq!(attempts, 1);
}

// ---------------------------------------------------------------------------
// No shell is involved
// ---------------------------------------------------------------------------

#[test]
fn shell_metacharacters_are_inert_arguments() {
    // If any of these were interpreted, the echoed argv would not match.
    let bins = FakeBins::new();
    let echo = bins.argv_echo();

    let hostile = [
        "; rm -rf /",
        "&& curl evil.example",
        "| tee /tmp/stolen",
        "$(whoami)",
        "`whoami`",
        "> /etc/passwd",
        "../../etc/passwd",
        "*",
        "~",
        "$HOME",
        "a\nb",
    ];

    let result = run(SafeCommand::new(echo.as_str(), bins.path()).args(hostile)).expect("run");

    assert!(result.success());
    let received: Vec<&str> = result.stdout.lines().collect();

    // `a\nb` arrives as one argument but prints as two lines, so the count is
    // compared against the expected line total rather than the argument total.
    assert_eq!(received.len(), hostile.len() + 1);
    assert_eq!(received[0], "; rm -rf /");
    assert_eq!(received[1], "&& curl evil.example");
    assert_eq!(received[3], "$(whoami)");
    // Not expanded: still the literal text.
    assert_eq!(received[9], "$HOME");
    assert_eq!(received[8], "~");
    assert_eq!(received[7], "*");
}

#[test]
fn arguments_with_spaces_stay_one_argument() {
    let bins = FakeBins::new();
    let echo = bins.argv_echo();

    let result = run(SafeCommand::new(echo.as_str(), bins.path())
        .arg("one argument with spaces")
        .arg("another one"))
    .expect("run");

    let received: Vec<&str> = result.stdout.lines().collect();
    assert_eq!(received, vec!["one argument with spaces", "another one"]);
}

#[test]
fn unicode_arguments_survive_intact() {
    let bins = FakeBins::new();
    let echo = bins.argv_echo();

    let values = ["Selamat datang 👋", "こんにちは", "ünïcödé — em-dash"];
    let result = run(SafeCommand::new(echo.as_str(), bins.path()).args(values)).expect("run");

    let received: Vec<&str> = result.stdout.lines().collect();
    assert_eq!(received, values);
}

#[test]
fn an_empty_argument_is_preserved() {
    let bins = FakeBins::new();
    let echo = bins.argv_echo();

    let result = run(SafeCommand::new(echo.as_str(), bins.path())
        .arg("before")
        .arg("")
        .arg("after"))
    .expect("run");

    assert_eq!(
        result.stdout.lines().collect::<Vec<_>>(),
        vec!["before", "", "after"]
    );
}

#[test]
fn the_working_directory_is_honoured() {
    let bins = FakeBins::new();
    let pwd = bins.write("show-cwd", "#!/bin/sh\npwd\n");

    let result = run(SafeCommand::new(pwd.as_str(), bins.path())).expect("run");

    assert!(
        result
            .stdout
            .trim()
            .ends_with(bins.path().file_name().unwrap())
    );
}

// ---------------------------------------------------------------------------
// Exit codes, timeouts, and missing tools
// ---------------------------------------------------------------------------

#[test]
fn a_non_zero_exit_is_reported_not_swallowed() {
    let bins = FakeBins::new();
    let failing = bins.write(
        "failing",
        "#!/bin/sh\necho 'something went wrong' >&2\nexit 3\n",
    );

    let result = run(SafeCommand::new(failing.as_str(), bins.path())).expect("run");

    assert!(!result.success());
    assert_eq!(result.exit_code, Some(3));
    assert!(result.stderr.contains("something went wrong"));
}

#[test]
fn run_checked_turns_a_failure_into_an_error_with_the_output() {
    let bins = FakeBins::new();
    let failing = bins.write("failing", "#!/bin/sh\necho 'the reason' >&2\nexit 1\n");

    let command = SafeCommand::new(failing.as_str(), bins.path());
    let error = with_spawn_retry(|| block_on(command.run_checked())).expect_err("must fail");

    assert!(matches!(error, ProcessError::Failed { .. }));
    // The repair surfaces the tool's own explanation rather than hiding it.
    assert!(error.repair().contains("the reason"));
}

#[test]
fn a_hung_process_is_stopped_at_the_timeout() {
    let bins = FakeBins::new();
    let hang = bins.write("hang", "#!/bin/sh\nsleep 30\n");

    let started = std::time::Instant::now();
    let error =
        run(SafeCommand::new(hang.as_str(), bins.path()).timeout(Duration::from_millis(300)))
            .expect_err("must time out");

    assert!(matches!(error, ProcessError::Timeout { .. }));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the timeout must actually cut the process short"
    );
    assert!(error.repair().contains("Try again"));
}

#[test]
fn a_missing_tool_is_reported_as_not_installed() {
    let bins = FakeBins::new();

    let error = run(SafeCommand::new(
        "kosong-definitely-not-a-real-tool",
        bins.path(),
    ))
    .expect_err("must fail");

    assert!(matches!(error, ProcessError::NotFound { .. }));
    assert!(error.repair().contains("Install"));
}

#[test]
fn a_child_cannot_read_from_the_terminal() {
    // stdin is null, so a tool that decides to prompt gets EOF and exits
    // rather than silently hanging the CLI forever.
    let bins = FakeBins::new();
    let reader = bins.write("reader", "#!/bin/sh\nread line || echo 'no input'\n");

    let result =
        run(SafeCommand::new(reader.as_str(), bins.path()).timeout(Duration::from_secs(5)))
            .expect("run");

    assert!(result.stdout.contains("no input"));
}

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

#[test]
fn github_token_shapes_are_redacted() {
    for token in [
        "ghp_16CharactersOrMoreOfTokenText",
        "gho_16CharactersOrMoreOfTokenText",
        "ghu_16CharactersOrMoreOfTokenText",
        "ghs_16CharactersOrMoreOfTokenText",
        "ghr_16CharactersOrMoreOfTokenText",
        "github_pat_11ABCDEFG0abcdefghijkl",
    ] {
        let output = redact(&format!("failed using {token} for auth"));
        assert!(!output.contains(token), "`{token}` leaked: {output}");
        assert!(output.contains(REDACTED));
    }
}

#[test]
fn tokens_are_redacted_even_when_punctuated() {
    let output = redact(r#"{"token":"ghp_16CharactersOrMoreOfText"}"#);
    assert!(!output.contains("ghp_16CharactersOrMoreOfText"));
}

#[test]
fn credential_keys_have_their_values_removed() {
    for line in [
        "authorization=abc123def456",
        "api_token=abc123def456",
        "Authorization: abc123def456",
        "access_token=abc123def456",
    ] {
        let output = redact(line);
        assert!(
            !output.contains("abc123def456"),
            "leaked in `{line}`: {output}"
        );
    }
}

#[test]
fn a_bearer_value_on_the_next_word_is_removed() {
    let output = redact("Authorization: Bearer abcdef1234567890");
    assert!(!output.contains("abcdef1234567890"), "leaked: {output}");
}

#[test]
fn slack_token_shapes_are_redacted() {
    // Written before narrowing the Slack prefix, because it had none: `xox`
    // was in the list and nothing anywhere asserted what it caught. A guard
    // against loosening the rule too far while fixing what it over-caught.
    //
    // The bodies follow the same convention as the GitHub fixtures above —
    // words, not digit groups. A first draft used realistic ones and GitHub's
    // push protection rejected the commit as a Slack token, which is a fair
    // objection to committing something shaped exactly like a credential. It
    // also costs the test nothing: redaction keys off the prefix and then runs
    // to the end of the credential characters, so what follows the hyphen is
    // never examined.
    for token in [
        "xoxb-16CharactersOrMoreOfTokenText",
        "xoxp-16CharactersOrMoreOfTokenText",
        "xoxa-16CharactersOrMoreOfTokenText",
        "xoxr-16CharactersOrMoreOfTokenText",
        "xoxs-16CharactersOrMoreOfTokenText",
        "xoxe-16CharactersOrMoreOfTokenText",
        "xoxc-16CharactersOrMoreOfTokenText",
        "xoxd-16CharactersOrMoreOfTokenText",
    ] {
        let output = redact(&format!("failed using {token} for auth"));
        assert!(!output.contains(token), "`{token}` leaked: {output}");
        assert!(output.contains(REDACTED));
    }
}

#[test]
fn a_name_that_merely_begins_like_a_token_is_not_one() {
    // The bug: `xox` matched anywhere in a line, so a Cloudflare Pages project
    // the user genuinely owns and named `xoxo-blog` was blanked out of
    // `wrangler pages project list`. `project_exists` then could not find it,
    // kosong tried to create a project that already existed, and told the user
    // to rename a name that was already theirs — a repair pointing the wrong
    // way, for a problem they did not have.
    //
    // `xoxo-` is not a Slack token type. Slack's are `xoxb-`, `xoxp-`, `xoxa-`,
    // `xoxe-`, `xoxr-`, `xoxs-`, `xoxc-` and `xoxd-`, all of which stay caught
    // above.
    let table = "\
│ Project Name    │ Project Domains         │
│ xoxo-blog       │ xoxo-blog.pages.dev     │";

    let output = redact(table);

    assert_eq!(output, table, "an ordinary project name was redacted");
    assert!(
        output.contains("xoxo-blog"),
        "the name must survive to be matched against: {output}"
    );
}

#[test]
fn a_credential_prefix_inside_a_word_is_not_a_credential() {
    // The same over-matching, from the other side: the scan started at every
    // byte, so a prefix landing mid-word took the rest of the word with it. A
    // credential does not begin halfway through one.
    for ordinary in [
        "the-ghp_naming-convention",
        "notxoxb-anything",
        "prefixglpat-suffix",
    ] {
        assert_eq!(
            redact(ordinary),
            ordinary,
            "`{ordinary}` was redacted mid-word"
        );
    }
}

#[test]
fn ordinary_output_survives_redaction_unchanged() {
    // Over-redaction makes tool output useless while protecting nothing. A
    // commit SHA is not a secret, and blanking it would break `git` output.
    let ordinary = "\
✓ Created repository owner/my-first-site on GitHub
  https://github.com/owner/my-first-site
  commit 9fc6b4312c0e1a2b3c4d5e6f708192a3b4c5d6e7
  Deployment complete! Take a peek over at https://abc123.my-site.pages.dev";

    assert_eq!(redact(ordinary), ordinary);
}

#[test]
fn redaction_preserves_line_structure() {
    let input = "line one\nline two\n\nline four\n";
    assert_eq!(redact(input), input);
}

#[test]
fn redaction_of_empty_text_is_empty() {
    assert_eq!(redact(""), "");
}

#[test]
fn process_output_is_redacted_before_a_caller_sees_it() {
    let bins = FakeBins::new();
    let leaky = bins.write(
        "leaky",
        "#!/bin/sh\necho 'error: bad credentials ghp_16CharactersOrMoreOfText'\n",
    );

    let result = run(SafeCommand::new(leaky.as_str(), bins.path())).expect("run");

    assert!(!result.stdout.contains("ghp_16CharactersOrMoreOfText"));
    assert!(result.stdout.contains(REDACTED));
    assert!(!result.combined_output().contains("ghp_16Characters"));
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

#[test]
fn a_displayed_command_quotes_what_needs_quoting() {
    let shown = format_command(
        "gh",
        &[
            "repo".into(),
            "create".into(),
            "my site".into(),
            "--private".into(),
        ],
    );
    assert_eq!(shown, "gh repo create 'my site' --private");
}

#[test]
fn a_displayed_command_redacts_a_token_shaped_argument() {
    let shown = format_command(
        "gh",
        &["auth".into(), "ghp_16CharactersOrMoreOfText".into()],
    );
    assert!(!shown.contains("ghp_16Characters"));
}

// ---------------------------------------------------------------------------
// The allowlist
// ---------------------------------------------------------------------------

#[test]
fn github_operations_build_the_expected_argv() {
    assert_eq!(GitHubOperation::AuthStatus.args(), vec!["auth", "status"]);
    assert_eq!(
        GitHubOperation::repo_view("my-site").unwrap().args(),
        vec!["repo", "view", "my-site"]
    );
    assert_eq!(
        GitHubOperation::repo_create("my-site", true, true)
            .unwrap()
            .args(),
        vec![
            "repo",
            "create",
            "my-site",
            "--private",
            "--source",
            ".",
            "--push"
        ]
    );
}

#[test]
fn visibility_is_always_stated_explicitly() {
    // Never left to `gh`'s own default, which the user's config could change.
    // Whether a page is public is not something to decide by omission.
    let private = GitHubOperation::repo_create("s", true, false)
        .unwrap()
        .args();
    let public = GitHubOperation::repo_create("s", false, false)
        .unwrap()
        .args();

    assert!(private.contains(&"--private".to_string()));
    assert!(public.contains(&"--public".to_string()));
}

#[test]
fn cloudflare_operations_build_the_expected_argv() {
    assert_eq!(CloudflareOperation::WhoAmI.args(), vec!["whoami"]);
    assert_eq!(
        CloudflareOperation::PagesProjectList.args(),
        vec!["pages", "project", "list"]
    );
    assert_eq!(
        CloudflareOperation::project_create("my-site", Some("main"))
            .unwrap()
            .args(),
        vec![
            "pages",
            "project",
            "create",
            "my-site",
            "--production-branch",
            "main"
        ]
    );
    // Without a branch the flag is absent entirely, not passed empty.
    assert_eq!(
        CloudflareOperation::project_create("my-site", None)
            .unwrap()
            .args(),
        vec!["pages", "project", "create", "my-site"]
    );
    assert_eq!(
        CloudflareOperation::deployment_list("my-site")
            .unwrap()
            .args(),
        vec!["pages", "deployment", "list", "--project-name", "my-site"]
    );
    assert_eq!(
        CloudflareOperation::deploy("my-site", Utf8Path::new("dist"), Some("main"))
            .unwrap()
            .args(),
        vec![
            "pages",
            "deploy",
            "dist",
            "--project-name",
            "my-site",
            "--branch",
            "main"
        ]
    );
}

#[test]
fn only_read_only_operations_are_non_mutating() {
    assert!(!GitHubOperation::AuthStatus.mutating());
    assert!(!GitHubOperation::repo_view("s").unwrap().mutating());
    assert!(
        GitHubOperation::repo_create("s", true, true)
            .unwrap()
            .mutating()
    );

    assert!(!CloudflareOperation::WhoAmI.mutating());
    assert!(!CloudflareOperation::PagesProjectList.mutating());
    assert!(
        !CloudflareOperation::deployment_list("s")
            .unwrap()
            .mutating()
    );
    assert!(
        CloudflareOperation::deploy("s", Utf8Path::new("dist"), None)
            .unwrap()
            .mutating()
    );
    assert!(
        CloudflareOperation::project_create("s", Some("main"))
            .unwrap()
            .mutating()
    );
}

#[test]
fn every_mutating_operation_states_its_remote_effect() {
    // §12.4 requires the remote effect be shown before confirmation, so a
    // mutating operation that cannot describe one is a bug.
    let mutating: Vec<Box<dyn Operation>> = vec![
        Box::new(GitHubOperation::repo_create("s", true, true).unwrap()),
        Box::new(GitHubOperation::RepoSync),
        Box::new(CloudflareOperation::deploy("s", Utf8Path::new("dist"), None).unwrap()),
        Box::new(CloudflareOperation::project_create("s", Some("main")).unwrap()),
    ];

    for operation in mutating {
        assert!(operation.mutating());
        let effect = operation.remote_effect();
        assert!(
            effect.as_deref().is_some_and(|e| !e.is_empty()),
            "`{}` is mutating but describes no remote effect",
            operation.summary()
        );
    }
}

#[test]
fn read_only_operations_claim_no_remote_effect() {
    assert_eq!(GitHubOperation::AuthStatus.remote_effect(), None);
    assert_eq!(CloudflareOperation::WhoAmI.remote_effect(), None);
}

#[test]
fn a_deploy_is_given_a_long_timeout() {
    let deploy = CloudflareOperation::deploy("s", Utf8Path::new("dist"), None).unwrap();
    assert_eq!(deploy.timeout(), LONG_TIMEOUT);
    assert!(CloudflareOperation::WhoAmI.timeout() < LONG_TIMEOUT);
}

// ---------------------------------------------------------------------------
// Name validation
// ---------------------------------------------------------------------------

#[test]
fn ordinary_names_are_accepted() {
    for name in ["my-first-site", "site1", "My_Site", "a"] {
        assert!(
            validate_name("project", name).is_ok(),
            "`{name}` should pass"
        );
    }
}

#[test]
fn traversal_and_injection_shaped_names_are_refused() {
    for name in [
        "../../etc/passwd",
        "../escape",
        "owner/repo/extra",
        "name;rm -rf /",
        "name with spaces",
        "name$(whoami)",
        "-leading-hyphen",
        "trailing-hyphen-",
        "",
        "  ",
    ] {
        let result = validate_name("project", name);
        assert!(
            matches!(result, Err(ProviderError::InvalidName { .. })),
            "`{name}` should be refused, got {result:?}"
        );
    }
}

#[test]
fn an_over_long_name_is_refused_with_its_own_message() {
    let long = "a".repeat(200);
    let error = validate_name("project", &long).unwrap_err();

    assert!(matches!(error, ProviderError::NameTooLong { .. }));
    assert!(error.repair().contains("Shorten"));
}

#[test]
fn an_owner_qualified_repository_is_accepted() {
    let operation = GitHubOperation::repo_view("khalilhimura/kosong").unwrap();
    assert_eq!(
        operation.args(),
        vec!["repo", "view", "khalilhimura/kosong"]
    );
}

#[test]
fn a_traversal_disguised_as_an_owner_is_refused() {
    assert!(GitHubOperation::repo_view("../../etc/passwd").is_err());
    assert!(GitHubOperation::repo_view("owner/../../etc").is_err());
}

// ---------------------------------------------------------------------------
// Plans and dry run
// ---------------------------------------------------------------------------

#[test]
fn a_plan_discloses_everything_the_specification_requires() {
    let cwd = Utf8Path::new("/tmp/site");
    let operation = CloudflareOperation::deploy("my-site", Utf8Path::new("dist"), None).unwrap();
    let plan = operation.plan(cwd);

    let disclosure = plan.disclosure().join("\n");

    // §12.4: executable, quoted argv, cwd, files, remote effect.
    assert!(disclosure.contains("wrangler pages deploy dist"));
    assert!(disclosure.contains("/tmp/site"));
    assert!(disclosure.contains("dist"));
    assert!(disclosure.contains("uploads your built files"));
    assert!(plan.mutating);
}

#[test]
fn a_read_only_plan_says_nothing_changes() {
    let plan = GitHubOperation::AuthStatus.plan(Utf8Path::new("/tmp"));
    let disclosure = plan.disclosure().join("\n");

    assert!(disclosure.contains("nothing outside this computer"));
    assert!(!plan.mutating);
}

#[test]
fn building_a_plan_does_not_invoke_the_executable() {
    // §12.5: a dry run must not run anything. The operation names a binary
    // that does not exist, so planning it would fail loudly if it executed.
    let bins = FakeBins::new();
    let marker = bins.path().join("was-executed");

    let sentinel = bins.write(
        "sentinel",
        &format!("#!/bin/sh\ntouch '{}'\n", marker.as_str()),
    );

    let command = SafeCommand::new(sentinel.as_str(), bins.path());
    let _shown = command.display();
    let _plan = GitHubOperation::AuthStatus.plan(bins.path());

    assert!(
        !marker.exists(),
        "planning or displaying a command must never execute it"
    );
}

// ---------------------------------------------------------------------------
// Interpreting provider output
// ---------------------------------------------------------------------------

#[test]
fn a_zero_exit_from_gh_means_signed_in() {
    assert_eq!(interpret_auth_status(Some(0), ""), AuthState::SignedIn);
}

#[test]
fn gh_reporting_logged_out_is_recognised() {
    assert_eq!(
        interpret_auth_status(Some(1), "You are not logged into any GitHub hosts."),
        AuthState::SignedOut
    );
    assert!(
        gh_repair(AuthState::SignedOut)
            .unwrap()
            .contains("gh auth login")
    );
}

#[test]
fn an_unexplained_gh_failure_is_not_guessed_at() {
    assert_eq!(
        interpret_auth_status(Some(1), "network unreachable"),
        AuthState::Unknown
    );
    assert!(gh_repair(AuthState::Unknown).is_some());
}

#[test]
fn wrangler_reporting_logged_out_despite_exit_zero_is_recognised() {
    use kosong_core::providers::{AuthState as CfAuth, cloudflare::interpret_whoami};

    // wrangler exits 0 while saying nobody is signed in, so the exit code
    // alone is not enough.
    assert_eq!(
        interpret_whoami(Some(0), "You are not authenticated."),
        CfAuth::SignedOut
    );
    assert_eq!(
        interpret_whoami(Some(0), "You are logged in with an API Token."),
        CfAuth::SignedIn
    );
}

// ---------------------------------------------------------------------------
// The rollback boundary
// ---------------------------------------------------------------------------

#[test]
fn rollback_guidance_is_honest_about_what_kosong_cannot_do() {
    use kosong_core::providers::cloudflare::rollback_guidance;

    let guidance = rollback_guidance("my-site");

    // Wrangler has no Pages rollback command, and using Cloudflare's REST API
    // would mean holding a token the CLI is designed never to hold. The
    // guidance must say so and offer a real route, not imply a capability.
    assert!(guidance.contains("dash.cloudflare.com"));
    assert!(guidance.contains("my-site"));
    assert!(guidance.contains("kosong site publish"));
    assert!(guidance.to_lowercase().contains("cannot"));
}

#[test]
fn cloudflare_rejects_names_cloudflare_itself_would_reject() {
    // Quoted from the Cloudflare API's own rejection (code 8000003), observed
    // against Wrangler 4.114.0: "Project names can be 1-58 lowercase
    // characters with dashes, but cannot start/end with a dash".
    //
    // This matters because `site init` slugifies, so kosong's validator is the
    // only gate on a hand-edited `.kosong/site.toml`. Without this the user
    // gets a raw provider error naming nothing they can act on.
    for good in ["my-site", "site2026", "a", "my-first-site"] {
        assert!(
            CloudflareOperation::project_create(good, Some("main")).is_ok(),
            "`{good}` is a valid Cloudflare project name"
        );
    }

    for bad in ["My_Site", "MySite", "my_site", "-leading", "trailing-"] {
        assert!(
            CloudflareOperation::project_create(bad, Some("main")).is_err(),
            "`{bad}` must be refused before it reaches wrangler"
        );
    }
}

#[test]
fn every_operation_carrying_a_project_name_applies_the_same_rule() {
    // One account-side constraint, so all three uses must agree. A name that
    // cannot be created also cannot be deployed to or listed.
    assert!(CloudflareOperation::project_create("My_Site", Some("main")).is_err());
    assert!(CloudflareOperation::deploy("My_Site", Utf8Path::new("dist"), None).is_err());
    assert!(CloudflareOperation::deployment_list("My_Site").is_err());
}

#[test]
fn a_refused_project_name_says_what_shape_is_wanted() {
    let error = CloudflareOperation::project_create("My_Site", Some("main")).unwrap_err();
    let repair = error.repair();

    assert!(repair.contains("lowercase"), "repair: {repair}");
    assert!(
        repair.contains("site.toml"),
        "repair must name where to fix it: {repair}"
    );
}

#[test]
fn a_create_result_is_read_for_which_kind_of_failure_it_was() {
    use kosong_core::providers::cloudflare::{ProjectCreateOutcome, interpret_project_create};

    assert_eq!(
        interpret_project_create(Some(0), "✨ Successfully created the 'my-site' project."),
        ProjectCreateOutcome::Created
    );

    // Pages project names are unique per account, so a second workspace with
    // the same name collides. That is a different problem from a broken
    // create, and the user can act on it.
    assert_eq!(
        interpret_project_create(Some(1), "A project with this name already exists"),
        ProjectCreateOutcome::NameTaken
    );
    assert_eq!(
        interpret_project_create(Some(1), "ERROR: project ALREADY EXISTS on this account"),
        ProjectCreateOutcome::NameTaken,
        "the match must not depend on casing"
    );

    // Pins the order the two checks run in: exit 0 alongside "already
    // exists" text must still read as a collision, not a success. If the
    // exit-code check ran first, this would come back `Created` instead —
    // exactly the misread that would send kosong on to deploy into a
    // project the user never chose. mirrors why `interpret_whoami` cannot
    // trust the exit code alone.
    assert_eq!(
        interpret_project_create(Some(0), "A project with this name already exists"),
        ProjectCreateOutcome::NameTaken,
        "text must be checked before the exit code, or a collision could be misread as success"
    );

    // Anything unrecognised keeps wrangler's own error rather than guessing.
    assert_eq!(
        interpret_project_create(Some(1), "Authentication error [code: 10000]"),
        ProjectCreateOutcome::Failed
    );
    assert_eq!(
        interpret_project_create(None, ""),
        ProjectCreateOutcome::Failed,
        "killed by a signal is a failure, not a collision"
    );
}

/// Ground truth for the `NameTaken` marker, which went six live smoke runs
/// without one.
///
/// Captured by provoking a real collision against Wrangler 4.114.0 — create a
/// throwaway Pages project, create it a second time, delete it — because every
/// other name on the development account is a live site, which is exactly why
/// this had never been done. The fixture below is what wrangler wrote to
/// stderr, byte for byte, with two substitutions: the account id and the log
/// path, neither of which belongs in a public repository and neither of which
/// the match depends on.
///
/// The escape codes are kept. They are the reason this test cannot be written
/// from the source: `contains` survives them only because they fall outside the
/// matched sentence, and nothing but a real capture shows where wrangler puts
/// them.
#[test]
fn a_real_wrangler_collision_is_read_as_a_taken_name() {
    use kosong_core::redact::redact;
    use kosong_core::providers::cloudflare::{ProjectCreateOutcome, interpret_project_create};

    let stderr = "\u{1b}[31m✘ \u{1b}[41;31m[\u{1b}[41;97mERROR\u{1b}[41;31m]\u{1b}[0m \u{1b}[1mA request to the Cloudflare API (/accounts/0123456789abcdef0123456789abcdef/pages/projects) failed.\u{1b}[0m\n\n  A project with this name already exists. Choose a different project name. [code: 8000002]\n  \n  If you think this is a bug, please open an issue at: \u{1b}[4mhttps://github.com/cloudflare/workers-sdk/issues/new/choose\u{1b}[0m\n\n\n🪵  Logs were written to \"/tmp/.wrangler/logs/wrangler-2026-07-28_18-32-30_718.log\"\n";

    assert_eq!(
        interpret_project_create(Some(1), stderr),
        ProjectCreateOutcome::NameTaken,
        "the real collision message must be recognised"
    );

    // The two markers must each carry the verdict alone. Cloudflare is free to
    // reword the sentence, and a future wrangler is free to stop printing the
    // code; neither should silently turn a collision back into a bare failure.
    assert_eq!(
        interpret_project_create(Some(1), "failed. [code: 8000002]"),
        ProjectCreateOutcome::NameTaken,
        "the API error code alone must be enough"
    );
    assert_eq!(
        interpret_project_create(Some(1), "A project with this name already exists."),
        ProjectCreateOutcome::NameTaken,
        "the prose alone must be enough"
    );

    // 8000003 is the *other* code this same call returns — a name Cloudflare
    // will not accept. Reading it as a collision would send the user to rename
    // a project over what is really a charset problem.
    assert_eq!(
        interpret_project_create(Some(1), "Validation error [code: 8000003]"),
        ProjectCreateOutcome::Failed,
        "an invalid name is not a taken name"
    );

    // The lesson of the `xoxo-blog` bug: this output is parsed, not just shown,
    // so redaction runs before the matcher ever sees it. A rule that blanked
    // the message would turn a collision into a confusing failure again.
    assert_eq!(
        interpret_project_create(Some(1), &redact(stderr)),
        ProjectCreateOutcome::NameTaken,
        "the message must survive redaction on its way out of the adapter"
    );
}

// ---------------------------------------------------------------------------
// Spawning an allowlisted program from where it was actually found
// ---------------------------------------------------------------------------

/// Cloudflare documents wrangler as a per-project dev dependency, so the binary
/// kosong must run often sits in the site's own `node_modules/.bin` rather than
/// on `PATH`. These pin the one mechanism that makes that possible: the program
/// stays the allowlisted *name*, and only the path spawned changes.
#[test]
fn a_command_spawns_the_path_it_was_found_at() {
    let bins = FakeBins::new();
    let found = bins.write("kosong-local-tool", "#!/bin/sh\nprintf 'ran\\n'\n");

    // The fake's directory is deliberately not on PATH, so a bare-name spawn
    // could not reach it. Succeeding proves the resolved path was used.
    let command = SafeCommand::new("kosong-local-tool", bins.path()).found_at(found);
    let result = with_spawn_retry(|| block_on(command.run())).expect("the resolved path must run");

    assert!(result.success());
    assert_eq!(result.stdout.trim(), "ran");
}

#[test]
fn a_program_not_on_path_and_not_resolved_is_still_not_found() {
    // The other half of the test above: without `found_at`, the same name fails.
    // Otherwise that test could pass for some unrelated reason.
    let bins = FakeBins::new();
    bins.write("kosong-local-tool", "#!/bin/sh\nprintf 'ran\\n'\n");

    let command = SafeCommand::new("kosong-local-tool", bins.path());

    assert!(matches!(
        block_on(command.run()),
        Err(ProcessError::NotFound { .. })
    ));
}

#[test]
fn a_resolved_command_reports_the_allowlisted_name_not_the_path() {
    // What the user reads must stay `wrangler`, not an absolute path into
    // node_modules. `perform_checked` builds "`{}` failed" from this field, and
    // §12.4's disclosure names the path separately.
    let bins = FakeBins::new();
    let found = bins.write("kosong-local-tool", "#!/bin/sh\nexit 3\n");

    let command = SafeCommand::new("kosong-local-tool", bins.path()).found_at(found);
    let result = with_spawn_retry(|| block_on(command.run())).expect("spawned");

    assert_eq!(result.exit_code, Some(3));
    assert_eq!(result.executable, "kosong-local-tool");
    assert_eq!(command.display(), "kosong-local-tool");
}

#[test]
fn a_disclosure_says_where_a_resolved_program_was_found() {
    // §12.4 requires the disclosure to describe what will actually run. With the
    // name alone it would be silent about which binary that is.
    let operation =
        CloudflareOperation::deploy("site", Utf8Path::new("dist"), None).expect("valid");
    let found = Utf8PathBuf::from("/tmp/site/node_modules/.bin/wrangler");

    let plan = operation
        .plan(Utf8Path::new("/tmp/site"))
        .found_at(found.clone());
    let disclosure = plan.disclosure().join("\n");

    assert!(
        disclosure.contains(found.as_str()),
        "the disclosure must name the binary being run: {disclosure}"
    );
    // The readable command line still reads as the tool, not as a path.
    assert!(disclosure.contains("wrangler pages deploy dist"));
}

#[test]
fn a_plan_with_nothing_resolved_says_nothing_extra() {
    // Every user without a project-local install must see exactly what they saw
    // before. This is what makes the change inert for them.
    let operation =
        CloudflareOperation::deploy("site", Utf8Path::new("dist"), None).expect("valid");

    let plan = operation.plan(Utf8Path::new("/tmp/site"));

    assert_eq!(plan.resolved, None);
    assert!(!plan.disclosure().join("\n").contains("node_modules"));
}

#[test]
fn the_cloudflare_repairs_use_the_documented_invocation() {
    // A project-local wrangler is not on PATH, so a repair reading "Run:
    // wrangler login" tells that user to run a command their shell cannot find
    // — which is the exact failure this whole change exists to remove.
    use kosong_core::providers::cloudflare::wrangler_repair;

    let signed_out = wrangler_repair(kosong_core::providers::AuthState::SignedOut)
        .expect("being signed out has a repair");
    assert!(
        signed_out.contains("npx wrangler login"),
        "repair: {signed_out}"
    );

    let unknown = wrangler_repair(kosong_core::providers::AuthState::Unknown)
        .expect("an unknown state has a repair");
    assert!(unknown.contains("npx wrangler whoami"), "repair: {unknown}");
}
