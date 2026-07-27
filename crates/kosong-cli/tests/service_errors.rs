//! What the CLI shows when the service answers a failure.
//!
//! The service can be up and still unable to do the thing asked of it — the
//! email provider being down is the case that prompted this. When it says so,
//! and says why, that sentence is the most useful thing on the screen, and it
//! must survive the trip to the user rather than being replaced by a request
//! id they can do nothing with.

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

/// A service that answers every request with one canned failure.
struct FailingApi {
    port: u16,
}

impl FailingApi {
    fn start(status: &'static str, body: &'static str) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let port = listener.local_addr().unwrap().port();
        let reply = Mutex::new((status, body));

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let (status, body) = *reply.lock().unwrap();
                let _ = answer(stream, status, body);
            }
        });

        Self { port }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

fn answer(mut stream: TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut line = String::new();
    reader.read_line(&mut line)?;

    let mut length = 0usize;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        if header.trim().is_empty() {
            break;
        }
        if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    if length > 0 {
        let mut discard = vec![0u8; length];
        reader.read_exact(&mut discard)?;
    }

    write!(
        stream,
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}

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

    fn run(&self, api: &FailingApi, args: &[&str]) -> Output {
        Command::new(binary())
            .arg("--workspace")
            .arg(&self.root)
            .args(args)
            .env("KOSONG_CONFIG_DIR", &self.config)
            .env("KOSONG_SESSION_FILE", self.config.join("session"))
            .env("KOSONG_API_URL", api.url())
            .env("NO_COLOR", "1")
            .output()
            .expect("run kosong")
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("process exited normally")
}

#[test]
fn a_service_that_says_why_it_cannot_help_has_that_reason_shown() {
    // The service returns 503 with a sentence a person can act on. Reporting
    // "the service had a problem, quote this id" instead would throw away the
    // only useful thing in the response.
    let api = FailingApi::start(
        "503 Service Unavailable",
        r#"{"code":"EMAIL_UNAVAILABLE","message":"Codes cannot be sent right now. Please try again in a few minutes.","request_id":"abc123"}"#,
    );
    let sandbox = Sandbox::new();

    let output = sandbox.run(&api, &["login", "--email", "person@example.com"]);
    let message = stderr(&output);

    // Exit 5 is the network/service code from the CLI contract.
    assert_eq!(code(&output), 5, "stderr: {message}");
    assert!(
        message.contains("Codes cannot be sent right now"),
        "the service's own explanation must reach the user: {message}"
    );
    assert!(
        !message.contains("abc123"),
        "a request id is not a next action, and should not stand in for one: {message}"
    );
}

#[test]
fn a_service_failure_with_nothing_to_say_still_gives_the_request_id() {
    // The other side of it. With no explanation there is nothing to show but
    // the identifier that lets someone look the failure up.
    let api = FailingApi::start(
        "500 Internal Server Error",
        r#"{"code":"INTERNAL_ERROR","message":"","request_id":"xyz789"}"#,
    );
    let sandbox = Sandbox::new();

    let output = sandbox.run(&api, &["login", "--email", "person@example.com"]);
    let message = stderr(&output);

    assert_eq!(code(&output), 5, "stderr: {message}");
    assert!(
        message.contains("xyz789"),
        "an unexplained failure should still be traceable: {message}"
    );
}
