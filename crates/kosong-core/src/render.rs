//! Markdown rendering with an explicit, testable safety policy.
//!
//! # Policy
//!
//! The technical specification requires that preview must not execute
//! JavaScript from document content, must not permit inline event handlers,
//! and must state its raw-HTML policy explicitly. This module implements that
//! as two rules:
//!
//! 1. **Raw HTML in the document is dropped, not rendered and not escaped
//!    into view.** This is a blunt instrument, chosen over sanitizing an
//!    allowlist of tags because a beginner-facing preview gains nothing from
//!    `<div>` support, and every sanitizer allowlist is a standing bet that
//!    the next parser bug is not exploitable. Dropping removes the entire
//!    class: no `<script>`, no `onclick=`, no `<iframe>`.
//!
//! 2. **Link and image URLs are checked against a scheme allowlist.** Dropping
//!    raw HTML alone is not enough, because `[click](javascript:alert(1))` is
//!    plain Markdown and would otherwise render as a live script URL.
//!
//! Both rules count what they removed, so the CLI can tell the user *"3 HTML
//! blocks were not rendered"* rather than silently changing their page. Being
//! honest about the transformation is part of the product's teaching promise.
//!
//! The rendered page also carries a restrictive Content-Security-Policy as
//! defence in depth. It is a second lock, not the first one.

use crate::okf::Document;
use pulldown_cmark::{CowStr, Event, Options, Parser, Tag};

/// URL schemes permitted in links and images.
pub const ALLOWED_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

/// What a blocked URL is rewritten to.
const BLOCKED_URL: &str = "#blocked-by-kosong";

/// Content-Security-Policy applied to the preview page.
///
/// `'unsafe-inline'` appears for styles only. The stylesheet is ours and is
/// inlined to keep preview a single self-contained response; scripts remain
/// fully denied.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; img-src 'self' https: data:; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'";

/// The result of rendering, including what the safety policy removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// The HTML document.
    pub html: String,
    /// Raw HTML fragments dropped by rule 1.
    pub stripped_html: usize,
    /// Link or image URLs neutralized by rule 2.
    pub blocked_urls: usize,
}

impl Rendered {
    /// Whether the safety policy changed anything, so the CLI knows to mention it.
    pub fn had_removals(&self) -> bool {
        self.stripped_html > 0 || self.blocked_urls > 0
    }

    /// A plain-language note about what was removed, if anything.
    ///
    /// Deliberately does not quote [`stripped_html`](Self::stripped_html) as a
    /// count. That field counts parser events, and a single `<b>x</b>` is two
    /// of them, so "2 HTML blocks" would be a confusing thing to tell someone
    /// who typed one tag. The count stays available for `--json` and
    /// diagnostics, where a raw number is the right currency.
    ///
    /// Blocked URLs *are* counted, because there one link really is one link.
    pub fn removal_note(&self) -> Option<String> {
        if !self.had_removals() {
            return None;
        }
        let mut parts = Vec::new();
        if self.stripped_html > 0 {
            parts.push("raw HTML was not shown, because preview renders Markdown only".to_owned());
        }
        if self.blocked_urls > 0 {
            let (link, pronoun) = if self.blocked_urls == 1 {
                ("link was", "it")
            } else {
                ("links were", "they")
            };
            parts.push(format!(
                "{} {link} turned off because {pronoun} could run code",
                self.blocked_urls
            ));
        }
        Some(format!("Preview safety: {}.", parts.join("; ")))
    }
}

/// Renders a Markdown body fragment to HTML, applying the safety policy.
///
/// Returns the body HTML only, without a surrounding page.
pub fn render_markdown(markdown: &str) -> Rendered {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);

    let mut stripped_html = 0usize;
    let mut blocked_urls = 0usize;
    // Depth of link tags whose destination was blocked, so the matching close
    // tag is dropped too and the output stays balanced.
    let mut events = Vec::new();

    for event in Parser::new_ext(markdown, options) {
        match event {
            // Rule 1. `Html` is a block, `InlineHtml` is inline, and
            // `DisplayMath`/`InlineMath` are not enabled. Dropping both HTML
            // variants is what removes scripts and event handlers.
            Event::Html(_) | Event::InlineHtml(_) => {
                stripped_html += 1;
            }

            // Rule 2, links.
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                let (url, blocked) = sanitize_url(dest_url);
                if blocked {
                    blocked_urls += 1;
                }
                events.push(Event::Start(Tag::Link {
                    link_type,
                    dest_url: url,
                    title,
                    id,
                }));
            }

            // Rule 2, images. A blocked image source becomes an empty src
            // rather than a fragment, so nothing is fetched.
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                let (url, blocked) = sanitize_url(dest_url);
                if blocked {
                    blocked_urls += 1;
                }
                events.push(Event::Start(Tag::Image {
                    link_type,
                    dest_url: if blocked { CowStr::Borrowed("") } else { url },
                    title,
                    id,
                }));
            }

            other => events.push(other),
        }
    }

    // Autolinks whose destination was blocked still carry the raw URL as their
    // link text, which is harmless: it renders as inert text.
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events.into_iter());

    Rendered {
        html,
        stripped_html,
        blocked_urls,
    }
}

/// Renders a whole document as a standalone HTML page.
pub fn render_page(document: &Document, filename: &str) -> Rendered {
    let title = document.title_or_filename(filename);
    let body = render_markdown(document.body());

    let html = format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="{csp}">
<title>{title}</title>
<style>{css}</style>
</head>
<body>
<main>
{body}
</main>
</body>
</html>
"#,
        csp = escape_html(CONTENT_SECURITY_POLICY),
        title = escape_html(&title),
        css = PREVIEW_CSS,
        body = body.html,
    );

    Rendered { html, ..body }
}

/// Checks a URL against the scheme allowlist.
///
/// Returns the URL to use and whether it was blocked.
///
/// Relative URLs and fragments have no scheme and are allowed. A URL with a
/// scheme is allowed only if that scheme is in [`ALLOWED_SCHEMES`].
fn sanitize_url(url: CowStr<'_>) -> (CowStr<'_>, bool) {
    if is_safe_url(&url) {
        (url, false)
    } else {
        (CowStr::Borrowed(BLOCKED_URL), true)
    }
}

/// Whether a URL is safe to emit.
///
/// Comparison happens after stripping ASCII whitespace and control characters,
/// because browsers ignore those when resolving a scheme: `java\tscript:` and
/// ` javascript:` both execute. Comparing the raw string would miss them.
pub fn is_safe_url(url: &str) -> bool {
    let normalized: String = url
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .collect::<String>()
        .to_ascii_lowercase();

    let Some(colon) = normalized.find(':') else {
        // No colon at all: relative path, fragment, or query. Safe.
        return true;
    };

    // A colon appearing after a path separator is part of the path, not a
    // scheme. `foo/bar:baz` is relative and safe.
    if let Some(slash) = normalized.find(['/', '?', '#']) {
        if slash < colon {
            return true;
        }
    }

    let scheme = &normalized[..colon];
    ALLOWED_SCHEMES.contains(&scheme)
}

/// Escapes text for interpolation into HTML.
pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Minimal readable stylesheet. Honours the reader's colour scheme.
const PREVIEW_CSS: &str = "\
*,*::before,*::after{box-sizing:border-box}
body{margin:0;padding:2.5rem 1.25rem;font:16px/1.65 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;color:#1a1a1a;background:#fff}
main{max-width:42rem;margin:0 auto}
h1,h2,h3{line-height:1.25;margin:2rem 0 .75rem}
h1{font-size:2rem;margin-top:0}
p,ul,ol,blockquote,table{margin:0 0 1rem}
a{color:#0b57d0}
code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.9em;background:#f2f2f2;padding:.15em .35em;border-radius:3px}
pre{background:#f6f6f6;padding:1rem;overflow-x:auto;border-radius:6px}
pre code{background:none;padding:0}
blockquote{border-left:3px solid #ddd;margin-left:0;padding-left:1rem;color:#555}
img{max-width:100%;height:auto}
table{border-collapse:collapse;width:100%;display:block;overflow-x:auto}
th,td{border:1px solid #ddd;padding:.5rem;text-align:left}
hr{border:0;border-top:1px solid #ddd;margin:2rem 0}
@media(prefers-color-scheme:dark){
body{color:#e8e8e8;background:#131313}
a{color:#8ab4f8}
code{background:#242424}
pre{background:#1c1c1c}
blockquote{border-left-color:#3a3a3a;color:#aaa}
th,td,hr{border-color:#333}
}";
