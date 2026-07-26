//! A temporary local website, served from the user's own computer.
//!
//! This is the lesson `kosong preview` teaches, so the implementation is
//! deliberately small enough to read: a socket, a loop, and one HTML response.
//!
//! # Safety properties
//!
//! - **Loopback only.** The listener binds `127.0.0.1`, never `0.0.0.0`. A
//!   preview of an unpublished draft must not be reachable from the coffee
//!   shop's network. [`Preview::bind`] offers no way to widen this.
//! - **No background processes.** Connections are handled inline on the
//!   calling thread; nothing is spawned. When the user presses Ctrl-C the
//!   process exits and nothing survives it.
//! - **Bounded work per connection.** Read and write timeouts plus a capped
//!   request size mean one stuck client cannot wedge the preview.
//! - **No document JavaScript.** Enforced upstream in [`crate::render`], and
//!   restated here as a `Content-Security-Policy` response header.

use crate::render::{self, Rendered};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

/// Default preview port.
pub const DEFAULT_PORT: u16 = 4173;

/// Largest request head accepted, in bytes.
const MAX_REQUEST_BYTES: usize = 8 * 1024;

/// Per-connection read and write timeout.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    #[error("port {port} is already being used by another program")]
    PortInUse { port: u16 },

    #[error("not allowed to use port {port}")]
    PortNotPermitted { port: u16 },

    #[error("could not start the preview server: {0}")]
    Io(#[from] io::Error),
}

impl PreviewError {
    /// A plain-language next action, in the CLI's voice.
    pub fn repair(&self) -> String {
        match self {
            Self::PortInUse { port } => format!(
                "Something else is already using port {port}. Try a different one:\n  kosong preview --port {}",
                port.saturating_add(1)
            ),
            Self::PortNotPermitted { port } => format!(
                "Ports below 1024 need administrator rights. Try a higher one:\n  kosong preview --port {DEFAULT_PORT}\n(you asked for {port})"
            ),
            Self::Io(_) => "Check that your network settings allow local connections.".into(),
        }
    }
}

/// A running preview server.
#[derive(Debug)]
pub struct Preview {
    listener: TcpListener,
    page: String,
}

impl Preview {
    /// Binds the preview server to loopback on `port`.
    ///
    /// Passing port `0` lets the operating system choose a free port, which is
    /// what the tests use to avoid depending on a fixed one being available.
    pub fn bind(port: u16, page: String) -> Result<Self, PreviewError> {
        // Ipv4Addr::LOCALHOST, not UNSPECIFIED. This is the whole safety
        // property, so it is written literally here and asserted in tests.
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));

        let listener = TcpListener::bind(address).map_err(|e| match e.kind() {
            io::ErrorKind::AddrInUse => PreviewError::PortInUse { port },
            io::ErrorKind::PermissionDenied => PreviewError::PortNotPermitted { port },
            _ => PreviewError::Io(e),
        })?;

        Ok(Self { listener, page })
    }

    /// Renders a document and binds a server for it.
    pub fn for_page(port: u16, rendered: &Rendered) -> Result<Self, PreviewError> {
        Self::bind(port, rendered.html.clone())
    }

    /// The address actually bound, after any OS port assignment.
    pub fn address(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// The URL to open, as the user should type it.
    pub fn url(&self) -> String {
        match self.address() {
            Ok(address) => format!("http://127.0.0.1:{}", address.port()),
            Err(_) => "http://127.0.0.1".to_owned(),
        }
    }

    /// Serves connections until the process is interrupted.
    ///
    /// Blocks the calling thread. Ctrl-C terminates the process, and because
    /// nothing was spawned, nothing is left running.
    pub fn serve_forever(&self) -> Result<(), PreviewError> {
        for stream in self.listener.incoming() {
            match stream {
                // One bad client must not end the preview.
                Ok(stream) => {
                    let _ = self.handle(stream);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => break,
                Err(_) => continue,
            }
        }
        Ok(())
    }

    /// Serves exactly one connection. Used by tests.
    pub fn serve_once(&self) -> Result<(), PreviewError> {
        let (stream, _) = self.listener.accept()?;
        self.handle(stream)?;
        Ok(())
    }

    fn handle(&self, stream: TcpStream) -> io::Result<()> {
        stream.set_read_timeout(Some(CONNECTION_TIMEOUT))?;
        stream.set_write_timeout(Some(CONNECTION_TIMEOUT))?;

        let request = read_request_line(&stream)?;
        let mut stream = stream;

        let Some((method, path)) = parse_request_line(&request) else {
            return write_response(
                &mut stream,
                400,
                "text/plain; charset=utf-8",
                b"Bad request",
            );
        };

        match method {
            "GET" | "HEAD" => {}
            _ => {
                return write_response(
                    &mut stream,
                    405,
                    "text/plain; charset=utf-8",
                    b"Only GET is supported",
                );
            }
        }

        // A browser requests this unprompted; answering "no content" keeps the
        // terminal free of confusing 404 noise.
        if path == "/favicon.ico" {
            return write_response(&mut stream, 204, "text/plain; charset=utf-8", b"");
        }

        // One document means every path shows the same page.
        let body = if method == "HEAD" {
            Vec::new()
        } else {
            self.page.as_bytes().to_vec()
        };
        write_response_with_len(
            &mut stream,
            200,
            "text/html; charset=utf-8",
            &body,
            self.page.len(),
        )
    }
}

/// Reads the request head, stopping at the blank line or the size cap.
fn read_request_line(stream: &TcpStream) -> io::Result<String> {
    // The cap wraps the stream before buffering, so the result still
    // implements BufRead and `read_line` remains available.
    let limited = stream.try_clone()?.take(MAX_REQUEST_BYTES as u64);
    let mut reader = BufReader::new(limited);
    let mut first = String::new();
    reader.read_line(&mut first)?;

    // Drain remaining headers so the client is not reset before it can read
    // the response. The values are unused; only the bytes need consuming.
    let mut line = String::new();
    while reader.read_line(&mut line)? > 0 {
        if line == "\r\n" || line == "\n" {
            break;
        }
        line.clear();
    }
    Ok(first)
}

/// Splits `GET /path HTTP/1.1` into method and path.
fn parse_request_line(line: &str) -> Option<(&str, &str)> {
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let path = parts.next()?;
    Some((method, path))
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    write_response_with_len(stream, status, content_type, body, body.len())
}

/// Writes a response, allowing the declared length to differ from the bytes
/// sent so `HEAD` can report the real size while sending no body.
fn write_response_with_len(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    content_length: usize,
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        405 => "Method Not Allowed",
        _ => "OK",
    };

    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {content_length}\r\n\
         Content-Security-Policy: {csp}\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Referrer-Policy: no-referrer\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\
         \r\n",
        csp = render::CONTENT_SECURITY_POLICY,
    );

    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}
