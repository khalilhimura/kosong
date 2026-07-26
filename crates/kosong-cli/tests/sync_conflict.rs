//! What `kosong sync --push` does when the server refuses the write.
//!
//! A 409 means someone wrote between the read that built the plan and the
//! write itself. The server's response carries only metadata — an etag, a
//! timestamp, a size — never the newer bytes, so the client cannot know what
//! the server now holds without asking again.
//!
//! That makes the conflict snapshot the load-bearing thing to get right. A
//! file labelled "server's version" that is not the server's version invites
//! the user to merge against something that no longer exists, and the merge
//! they push would silently drop whatever the other machine wrote.
//!
//! These tests drive the real binary against a scripted HTTP server, because
//! the race cannot be produced against a real one on demand.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::Mutex;
use tempfile::TempDir;

fn binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("test binary path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("kosong")
}

/// One canned HTTP response.
struct Reply {
    status: &'static str,
    body: String,
}

impl Reply {
    fn ok(body: impl Into<String>) -> Self {
        Self {
            status: "200 OK",
            body: body.into(),
        }
    }

    fn document(etag: &str, contents: &str) -> Self {
        Self::ok(format!(
            r#"{{"etag":"{etag}","updated_at":"2026-07-26T10:00:00Z","document":"{}"}}"#,
            BASE64.encode(contents.as_bytes())
        ))
    }

    fn conflict(remote_etag: &str) -> Self {
        Self {
            status: "409 Conflict",
            body: format!(
                r#"{{"code":"DOCUMENT_CONFLICT","message":"the page on the server changed",
                     "remote_etag":"{remote_etag}","remote_updated_at":"2026-07-26T10:05:00Z",
                     "remote_byte_size":42}}"#
            ),
        }
    }

    fn not_found() -> Self {
        Self {
            status: "404 Not Found",
            body: r#"{"code":"NOT_FOUND","message":"no document"}"#.to_owned(),
        }
    }
}

/// An API that answers from a script rather than from state.
///
/// Document reads are served in order, so a test can say "the first read sees
/// this, the read after the refused write sees that" — which is the whole
/// shape of the race being reproduced.
struct FakeApi {
    port: u16,
}

impl FakeApi {
    fn start(document_reads: Vec<Reply>, put_reply: Reply) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind fake api");
        let port = listener.local_addr().unwrap().port();

        let reads = Mutex::new(document_reads);
        let put = Mutex::new(Some(put_reply));

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let _ = handle(stream, &reads, &put);
            }
        });

        Self { port }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

/// Serves one request, dispatching on method and path.
fn handle(
    mut stream: TcpStream,
    reads: &Mutex<Vec<Reply>>,
    put: &Mutex<Option<Reply>>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    let mut content_length = 0usize;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        if header.trim().is_empty() {
            break;
        }
        if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }

    // The body has to be drained even when it is ignored, or the client sees
    // the connection close mid-write rather than the scripted status.
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body)?;
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();

    let reply = match (method, path) {
        ("POST", "/v1/auth/refresh") => {
            Reply::ok(r#"{"access_token":"access","refresh_token":"refresh"}"#)
        }
        ("GET", "/v1/document") => {
            let mut remaining = reads.lock().unwrap();
            if remaining.is_empty() {
                Reply::not_found()
            } else {
                remaining.remove(0)
            }
        }
        ("PUT", "/v1/document") => put.lock().unwrap().take().unwrap_or_else(|| {
            Reply::ok(r#"{"etag":"written","updated_at":"2026-07-26T10:10:00Z"}"#)
        }),
        _ => Reply::not_found(),
    };

    write!(
        stream,
        "HTTP/1.1 {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        reply.status,
        reply.body.len(),
        reply.body
    )?;
    stream.flush()
}

/// A workspace, an isolated config, and a planted sign-in.
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
        let sandbox = Self {
            _guard: guard,
            root,
            config,
        };
        std::fs::write(sandbox.session_file(), "a-stored-refresh-token").expect("session");
        sandbox
    }

    fn session_file(&self) -> PathBuf {
        self.config.join("session")
    }

    fn run(&self, api: &FakeApi, args: &[&str]) -> Output {
        Command::new(binary())
            .arg("--workspace")
            .arg(&self.root)
            .args(args)
            .env("KOSONG_CONFIG_DIR", &self.config)
            .env("KOSONG_SESSION_FILE", self.session_file())
            .env("KOSONG_API_URL", api.url())
            .env("NO_COLOR", "1")
            .output()
            .expect("run kosong")
    }

    /// Creates the document with `kosong new`, so the push sends something the
    /// real writer produced rather than a fixture that might drift from it.
    fn write_document(&self, title: &str) {
        let output = Command::new(binary())
            .arg("--workspace")
            .arg(&self.root)
            .args(["new", "--title", title])
            .env("KOSONG_CONFIG_DIR", &self.config)
            .env("KOSONG_SESSION_FILE", self.session_file())
            .env("NO_COLOR", "1")
            .output()
            .expect("run kosong new");
        assert!(
            output.status.success(),
            "could not create the document: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// The two files a conflict leaves behind, if it left any.
    fn conflict_snapshot(&self) -> Option<(String, String)> {
        let conflicts = self.root.join(".kosong").join("conflicts");
        let entry = std::fs::read_dir(conflicts).ok()?.next()?.ok()?;
        let yours = std::fs::read_to_string(entry.path().join("your-version.md")).ok()?;
        let theirs = std::fs::read_to_string(entry.path().join("server-version.md")).ok()?;
        Some((yours, theirs))
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("process exited normally")
}

#[test]
fn a_refused_write_snapshots_the_version_that_is_actually_on_the_server() {
    // The read that builds the plan sees v1. Another machine writes v2. The
    // push is refused. The user must be shown v2 — showing them v1 would have
    // them merge against a version nobody holds any more, and pushing that
    // merge would drop v2 without ever displaying it.
    let sandbox = Sandbox::new();
    sandbox.write_document("Mine");

    let api = FakeApi::start(
        vec![
            Reply::document("etag-v1", "the version this push was based on"),
            Reply::document("etag-v2", "what the other machine wrote"),
        ],
        Reply::conflict("etag-v2"),
    );

    let output = sandbox.run(&api, &["sync", "--push"]);

    assert_eq!(code(&output), 2, "stderr: {}", stderr(&output));

    let (yours, theirs) = sandbox
        .conflict_snapshot()
        .expect("both versions must be preserved");

    assert!(
        yours.contains("Mine"),
        "your version must be yours: {yours}"
    );
    assert!(
        theirs.contains("what the other machine wrote"),
        "the snapshot must hold the server's current version, not the one the \
         push was based on: {theirs}"
    );
    assert!(
        !theirs.contains("the version this push was based on"),
        "the snapshot is stale: it holds the pre-push read, not what is on the \
         server now"
    );
}

#[test]
fn a_conflict_whose_other_side_cannot_be_read_is_reported_not_invented() {
    // The server refuses the write and then reports no document at all. There
    // is nothing truthful to put in a "server's version" file, so the command
    // must say so rather than write an empty one and call it the server's.
    let sandbox = Sandbox::new();
    sandbox.write_document("Mine");

    let api = FakeApi::start(
        vec![Reply::document(
            "etag-v1",
            "the version this push was based on",
        )],
        Reply::conflict("etag-v2"),
    );

    let output = sandbox.run(&api, &["sync", "--push"]);

    assert_ne!(code(&output), 0, "a refused write is not a success");

    assert!(
        sandbox.conflict_snapshot().is_none(),
        "with nothing truthful to put in it, no `server's version` file should \
         be written at all — an empty or stale one reads as authoritative"
    );

    let message = stderr(&output).to_lowercase();
    assert!(
        message.contains("server"),
        "the failure must explain what happened: {message}"
    );
    assert!(
        std::fs::read_to_string(sandbox.root.join("kosong.md"))
            .unwrap()
            .contains("Mine"),
        "the local page must be untouched"
    );
}
