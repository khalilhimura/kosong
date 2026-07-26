//! Preview server behaviour.
//!
//! The load-bearing test here is [`the_listener_binds_only_to_loopback`].
//! Technical specification §14 lists "insecure preview exposure" as a threat
//! with the control "loopback-only server" and the test "assert listener
//! rejects non-loopback binding".

use kosong_core::preview::{DEFAULT_PORT, Preview, PreviewError};
use kosong_core::render;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpStream};

/// Binds a preview on an OS-assigned port so tests never fight over 4173.
fn preview(page: &str) -> Preview {
    Preview::bind(0, page.to_owned()).expect("bind to an ephemeral port")
}

/// Sends a raw request and returns the full response, serving one connection.
fn round_trip(server: &Preview, request: &str) -> String {
    let port = server.address().unwrap().port();
    let request = request.to_owned();

    let client = std::thread::spawn(move || {
        let mut stream =
            TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connect to preview");
        stream.write_all(request.as_bytes()).unwrap();
        stream.flush().unwrap();

        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    });

    server.serve_once().expect("serve one connection");
    client.join().expect("client thread")
}

// ---------------------------------------------------------------------------
// Binding
// ---------------------------------------------------------------------------

#[test]
fn the_listener_binds_only_to_loopback() {
    let server = preview("<h1>hi</h1>");
    let address = server.address().unwrap();

    assert_eq!(
        address.ip(),
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        "preview must never be reachable from another machine"
    );
    assert!(address.ip().is_loopback());
    assert_ne!(address.ip(), IpAddr::V4(Ipv4Addr::UNSPECIFIED));
}

#[test]
fn the_url_points_at_loopback() {
    let server = preview("<h1>hi</h1>");
    assert!(server.url().starts_with("http://127.0.0.1:"));
}

#[test]
fn an_occupied_port_produces_an_actionable_error() {
    let first = preview("<h1>hi</h1>");
    let port = first.address().unwrap().port();

    let err = Preview::bind(port, "<h1>second</h1>".into()).unwrap_err();

    assert!(matches!(err, PreviewError::PortInUse { .. }));
    let repair = err.repair();
    assert!(repair.contains("--port"), "repair must suggest a way out");
    assert!(
        repair.contains(&(port + 1).to_string()),
        "repair should suggest a concrete alternative port: {repair}"
    );
}

#[test]
fn the_default_port_matches_the_specification() {
    assert_eq!(DEFAULT_PORT, 4173);
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

#[test]
fn a_get_request_returns_the_page() {
    let server = preview("<h1>My First Site</h1>");
    let response = round_trip(&server, "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");

    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.contains("<h1>My First Site</h1>"));
    assert!(response.contains("Content-Type: text/html; charset=utf-8"));
}

#[test]
fn responses_carry_the_content_security_policy() {
    let server = preview("<h1>hi</h1>");
    let response = round_trip(&server, "GET / HTTP/1.1\r\n\r\n");

    assert!(response.contains("Content-Security-Policy:"));
    assert!(response.contains(render::CONTENT_SECURITY_POLICY));
    assert!(response.contains("X-Content-Type-Options: nosniff"));
    assert!(response.contains("Referrer-Policy: no-referrer"));
}

#[test]
fn the_content_length_matches_the_page() {
    let page = "<h1>exact</h1>";
    let server = preview(page);
    let response = round_trip(&server, "GET / HTTP/1.1\r\n\r\n");

    assert!(response.contains(&format!("Content-Length: {}", page.len())));
}

#[test]
fn every_path_serves_the_one_document() {
    // kosong manages a single document, so there is nothing else to route to.
    for path in ["/", "/anything", "/deeply/nested/path"] {
        let server = preview("<h1>only page</h1>");
        let response = round_trip(&server, &format!("GET {path} HTTP/1.1\r\n\r\n"));

        assert!(
            response.contains("<h1>only page</h1>"),
            "path {path} failed"
        );
    }
}

#[test]
fn a_favicon_request_is_answered_without_noise() {
    let server = preview("<h1>hi</h1>");
    let response = round_trip(&server, "GET /favicon.ico HTTP/1.1\r\n\r\n");

    assert!(response.starts_with("HTTP/1.1 204 No Content"));
}

#[test]
fn head_reports_the_size_without_sending_the_body() {
    let page = "<h1>body content here</h1>";
    let server = preview(page);
    let response = round_trip(&server, "HEAD / HTTP/1.1\r\n\r\n");

    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.contains(&format!("Content-Length: {}", page.len())));
    assert!(
        !response.contains("body content here"),
        "HEAD must not send a body"
    );
}

#[test]
fn a_write_method_is_refused() {
    let server = preview("<h1>hi</h1>");
    let response = round_trip(&server, "POST / HTTP/1.1\r\nContent-Length: 0\r\n\r\n");

    assert!(response.starts_with("HTTP/1.1 405"));
    assert!(!response.contains("<h1>hi</h1>"));
}

#[test]
fn a_malformed_request_does_not_crash_the_server() {
    let server = preview("<h1>hi</h1>");
    let response = round_trip(&server, "not-a-real-request\r\n\r\n");

    assert!(response.starts_with("HTTP/1.1 400"));
}

#[test]
fn an_oversized_request_head_does_not_take_the_server_down() {
    // A client sending a huge header must not exhaust memory or hang the
    // preview. The read is capped at 8 KiB, so the server stops reading,
    // answers, and closes while the client is still sending — which resets
    // that connection. That reset is the cap working, not a failure, so the
    // property under test is survival: the *next* request must still succeed.
    let server = preview("<h1>hi</h1>");
    let port = server.address().unwrap().port();

    let flood = std::thread::spawn(move || {
        let Ok(mut stream) = TcpStream::connect((Ipv4Addr::LOCALHOST, port)) else {
            return;
        };
        let head = "GET / HTTP/1.1\r\nX-Filler: ".to_owned() + &"a".repeat(64 * 1024);
        // Both the write and the read may be reset. Either is acceptable.
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.read_to_string(&mut String::new());
    });

    let _ = server.serve_once();
    flood.join().expect("flood client thread");

    let response = round_trip(&server, "GET / HTTP/1.1\r\n\r\n");
    assert!(
        response.starts_with("HTTP/1.1 200 OK"),
        "the server must still serve after a flooded connection, got: {}",
        response.chars().take(60).collect::<String>()
    );
}

// ---------------------------------------------------------------------------
// Integration with rendering
// ---------------------------------------------------------------------------

#[test]
fn a_rendered_document_is_served_end_to_end() {
    // The smoke assertion from technical specification §7.
    let document = kosong_core::okf::Document::new("My First Site").unwrap();
    let rendered = render::render_page(&document, "kosong.md");
    let server = Preview::for_page(0, &rendered).unwrap();

    let response = round_trip(&server, "GET / HTTP/1.1\r\n\r\n");

    assert!(response.contains("My First Site"));
    assert!(response.contains("<!doctype html>"));
}
