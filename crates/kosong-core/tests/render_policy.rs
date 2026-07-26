//! The rendering safety policy.
//!
//! Technical specification §7 requires that preview must not execute
//! JavaScript from document content, must not permit inline event handlers,
//! and must state its raw-HTML policy explicitly and test it.
//!
//! These tests are the statement of that policy.

use kosong_core::okf::Document;
use kosong_core::render::{self, render_markdown, render_page};

// ---------------------------------------------------------------------------
// Rule 1: raw HTML is dropped
// ---------------------------------------------------------------------------

#[test]
fn script_tags_are_not_rendered() {
    let out = render_markdown("Before\n\n<script>alert('xss')</script>\n\nAfter\n");

    assert!(!out.html.contains("<script"));
    assert!(!out.html.contains("alert"));
    assert!(out.html.contains("Before"));
    assert!(out.html.contains("After"));
    assert!(out.stripped_html > 0);
}

#[test]
fn inline_event_handlers_are_not_rendered() {
    let out = render_markdown("Text with <span onclick=\"steal()\">a trap</span> inside.\n");

    assert!(!out.html.contains("onclick"));
    assert!(!out.html.contains("steal"));
    assert!(out.stripped_html > 0);
}

#[test]
fn iframes_and_objects_are_not_rendered() {
    for payload in [
        "<iframe src=\"https://evil.example\"></iframe>",
        "<object data=\"x\"></object>",
        "<embed src=\"x\">",
        "<svg onload=\"alert(1)\"></svg>",
    ] {
        let out = render_markdown(&format!("Before\n\n{payload}\n\nAfter\n"));
        assert!(
            !out.html.contains("iframe")
                && !out.html.contains("onload")
                && !out.html.contains("<object")
                && !out.html.contains("<embed"),
            "`{payload}` leaked into output:\n{}",
            out.html
        );
    }
}

#[test]
fn html_inside_a_code_block_is_shown_as_text_not_executed() {
    // A fenced code block is content, not markup. It must be escaped and
    // displayed, which is different from being dropped.
    let out = render_markdown("```html\n<script>alert(1)</script>\n```\n");

    assert!(
        out.html.contains("&lt;script&gt;"),
        "code samples should stay visible, escaped:\n{}",
        out.html
    );
    assert!(!out.html.contains("<script>"));
}

#[test]
fn ordinary_markdown_is_untouched() {
    let out = render_markdown("# Title\n\nSome **bold** and *italic* text.\n\n- one\n- two\n");

    assert!(out.html.contains("<h1>Title</h1>"));
    assert!(out.html.contains("<strong>bold</strong>"));
    assert!(out.html.contains("<li>one</li>"));
    assert!(
        !out.had_removals(),
        "clean Markdown must report no removals"
    );
}

#[test]
fn tables_and_task_lists_render() {
    let out = render_markdown("| A | B |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n- [ ] todo\n");

    assert!(out.html.contains("<table>"));
    assert!(out.html.contains("<td>1</td>"));
    assert!(out.html.contains("type=\"checkbox\""));
}

// ---------------------------------------------------------------------------
// Rule 2: URL scheme allowlist
// ---------------------------------------------------------------------------

#[test]
fn javascript_urls_are_blocked() {
    let out = render_markdown("[click me](javascript:alert(1))\n");

    assert!(!out.html.contains("javascript:"));
    assert_eq!(out.blocked_urls, 1);
    // The link text survives; only the destination is neutralized.
    assert!(out.html.contains("click me"));
}

#[test]
fn javascript_urls_are_blocked_regardless_of_disguise() {
    // Browsers ignore case, leading whitespace, and embedded control
    // characters when resolving a scheme. So must the check.
    let disguises = [
        "JavaScript:alert(1)",
        "JAVASCRIPT:alert(1)",
        " javascript:alert(1)",
        "java\tscript:alert(1)",
        "java\nscript:alert(1)",
        "jav\u{0000}ascript:alert(1)",
    ];

    for disguise in disguises {
        assert!(
            !render::is_safe_url(disguise),
            "`{}` should be treated as a javascript URL",
            disguise.escape_debug()
        );
    }
}

#[test]
fn other_dangerous_schemes_are_blocked() {
    for scheme in [
        "data:text/html;base64,PHNjcmlwdD4=",
        "vbscript:msgbox(1)",
        "file:///etc/passwd",
    ] {
        assert!(!render::is_safe_url(scheme), "`{scheme}` should be blocked");
    }
}

#[test]
fn ordinary_urls_are_allowed() {
    for url in [
        "https://example.com",
        "http://example.com/path?query=1",
        "mailto:person@example.com",
        "/absolute/bundle/relative.md",
        "relative/path.md",
        "../sibling.md",
        "#fragment",
        "",
        // A colon inside a path segment is not a scheme.
        "path/with:colon.md",
    ] {
        assert!(render::is_safe_url(url), "`{url}` should be allowed");
    }
}

#[test]
fn a_blocked_image_source_is_emptied_so_nothing_is_fetched() {
    let out = render_markdown("![alt](javascript:alert(1))\n");

    assert!(!out.html.contains("javascript:"));
    assert!(
        !out.html.contains("#blocked"),
        "an image src must be emptied, not pointed at a fragment:\n{}",
        out.html
    );
    assert_eq!(out.blocked_urls, 1);
}

#[test]
fn safe_links_are_not_counted_as_blocked() {
    let out = render_markdown("[ok](https://example.com) and [also ok](/relative.md)\n");

    assert_eq!(out.blocked_urls, 0);
    assert!(out.html.contains("https://example.com"));
}

// ---------------------------------------------------------------------------
// Honest reporting
// ---------------------------------------------------------------------------

#[test]
fn removals_are_reported_in_plain_language() {
    let out = render_markdown("<script>x</script>\n\n[a](javascript:x)\n");

    assert!(out.had_removals());
    let note = out.removal_note().expect("a note when things were removed");
    assert!(note.contains("raw HTML"));
    assert!(note.contains("link"));
    // The product forbids jargon in user-facing copy.
    assert!(!note.to_lowercase().contains("sanitiz"));
    assert!(!note.to_lowercase().contains("xss"));
}

#[test]
fn clean_documents_produce_no_note() {
    assert_eq!(render_markdown("# Just a heading\n").removal_note(), None);
}

#[test]
fn the_note_uses_singular_and_plural_correctly_for_links() {
    let one = render_markdown("[a](javascript:x)\n");
    assert!(one.removal_note().unwrap().contains("1 link was"));

    let many = render_markdown("[a](javascript:x) [b](vbscript:y)\n");
    assert!(many.removal_note().unwrap().contains("2 links were"));
}

#[test]
fn the_note_does_not_quote_a_confusing_html_fragment_count() {
    // A single inline tag produces two parser events (open and close).
    // Reporting "2 HTML blocks" to someone who typed one tag would be wrong,
    // so the note describes the removal without a number.
    let out = render_markdown("<b>x</b>\n");

    assert_eq!(out.stripped_html, 2, "the raw count is still available");

    let note = out.removal_note().unwrap();
    assert!(note.contains("raw HTML was not shown"));
    assert!(
        !note.contains('2'),
        "the note must not quote the event count: {note}"
    );
}

// ---------------------------------------------------------------------------
// Full page
// ---------------------------------------------------------------------------

#[test]
fn a_rendered_page_is_a_complete_html_document() {
    let document = Document::new("My First Site").unwrap();
    let out = render_page(&document, "kosong.md");

    assert!(out.html.starts_with("<!doctype html>"));
    assert!(out.html.contains("<html lang=\"en\">"));
    assert!(out.html.contains("<meta charset=\"utf-8\">"));
    assert!(out.html.contains("<title>My First Site</title>"));
    assert!(out.html.contains("<h1>My First Site</h1>"));
    assert!(out.html.trim_end().ends_with("</html>"));
}

#[test]
fn a_rendered_page_carries_a_content_security_policy() {
    let document = Document::new("Test").unwrap();
    let out = render_page(&document, "kosong.md");

    assert!(out.html.contains("Content-Security-Policy"));
    // Scripts must be denied outright, as defence in depth behind rule 1.
    assert!(render::CONTENT_SECURITY_POLICY.contains("default-src 'none'"));
    assert!(!render::CONTENT_SECURITY_POLICY.contains("script-src 'unsafe-inline'"));
}

#[test]
fn a_hostile_title_cannot_break_out_of_the_title_tag() {
    let source = "---\ntype: Page\ntitle: \"</title><script>alert(1)</script>\"\n---\nbody\n";
    let document = Document::parse(source).unwrap();

    let out = render_page(&document, "kosong.md");

    assert!(
        !out.html.contains("<script>"),
        "the title must be escaped:\n{}",
        out.html
    );
    assert!(out.html.contains("&lt;/title&gt;"));
}

#[test]
fn a_page_without_a_title_falls_back_to_the_filename() {
    let document = Document::parse("---\ntype: Page\n---\n# Body heading\n").unwrap();
    let out = render_page(&document, "kosong.md");

    assert!(out.html.contains("<title>kosong</title>"));
}

#[test]
fn page_rendering_keeps_the_body_removal_counts() {
    let source = "---\ntype: Page\ntitle: T\n---\n<script>x</script>\n\n[a](javascript:x)\n";
    let document = Document::parse(source).unwrap();

    let out = render_page(&document, "kosong.md");

    assert!(out.stripped_html > 0);
    assert_eq!(out.blocked_urls, 1);
}

// ---------------------------------------------------------------------------
// Escaping
// ---------------------------------------------------------------------------

#[test]
fn escape_html_covers_every_dangerous_character() {
    assert_eq!(
        render::escape_html(r#"<a href="x" onclick='y'>&</a>"#),
        "&lt;a href=&quot;x&quot; onclick=&#39;y&#39;&gt;&amp;&lt;/a&gt;"
    );
}

#[test]
fn escape_html_leaves_ordinary_text_alone() {
    assert_eq!(
        render::escape_html("Selamat datang 👋"),
        "Selamat datang 👋"
    );
}
