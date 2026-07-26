//! Conformance tests for Open Knowledge Format v0.1.
//!
//! These map one-to-one onto the checklist in `spec/okf-profile-v1.md` §7.
//!
//! The tests that matter most are the tolerance ones. OKF explicitly forbids a
//! consumer from rejecting a bundle for missing optional fields, unknown `type`
//! values, unknown keys, broken links, or a missing `index.md`. A consumer that
//! is merely strict is non-conformant, so "this parses" is as much a
//! requirement as "this is rejected".

use kosong_core::okf::{
    self, DEFAULT_TYPE, Document, LineEnding, MAX_TITLE_CHARS, OkfError, Visibility,
};

// ---------------------------------------------------------------------------
// Conformance criterion 2: `type` is required and must be non-empty
// ---------------------------------------------------------------------------

#[test]
fn document_with_only_type_is_conformant() {
    // The minimum legal OKF document. Nothing but `type` and a body.
    let doc = Document::parse("---\ntype: Page\n---\n# Hello\n").expect("minimal doc must parse");

    assert_eq!(doc.doc_type(), "Page");
    assert_eq!(doc.body(), "# Hello\n");
    assert_eq!(doc.title(), None);
    assert!(!doc.is_managed());
}

#[test]
fn missing_type_is_rejected_with_a_repair_suggestion() {
    let err = Document::parse("---\ntitle: No type here\n---\nbody\n").unwrap_err();

    assert!(matches!(err, OkfError::MissingType));
    assert!(err.repair().contains("type:"));
}

#[test]
fn empty_and_whitespace_only_type_are_rejected() {
    for front in ["type: \"\"", "type: \"   \"", "type:"] {
        let source = format!("---\n{front}\n---\nbody\n");
        let err =
            Document::parse(&source).unwrap_err_or_panic(&format!("`{front}` must be rejected"));
        assert!(
            matches!(err, OkfError::EmptyType | OkfError::MissingType),
            "`{front}` produced {err:?}"
        );
    }
}

#[test]
fn absent_front_matter_is_rejected() {
    let err = Document::parse("# Just markdown\n").unwrap_err();
    assert!(matches!(err, OkfError::MissingFrontMatter));
    assert!(!err.repair().is_empty());
}

#[test]
fn unterminated_front_matter_is_rejected() {
    let err = Document::parse("---\ntype: Page\nnever closed\n").unwrap_err();
    assert!(matches!(err, OkfError::UnterminatedFrontMatter));
}

// ---------------------------------------------------------------------------
// Consumer obligations: things that must NOT be errors
// ---------------------------------------------------------------------------

#[test]
fn unrecognised_type_values_are_accepted() {
    // OKF types are not centrally registered. A consumer must tolerate any
    // value, including ones invented by a producer it has never seen.
    for type_value in [
        "BigQuery Table",
        "Metric",
        "Playbook",
        "Attested Computation",
        "something-nobody-has-defined",
    ] {
        let source = format!("---\ntype: {type_value}\n---\nbody\n");
        let doc = Document::parse(&source)
            .unwrap_or_else(|e| panic!("type `{type_value}` must be accepted, got {e:?}"));
        assert_eq!(doc.doc_type(), type_value);
    }
}

#[test]
fn broken_cross_links_are_accepted() {
    // Broken links may represent knowledge not yet written.
    let doc = Document::parse(
        "---\ntype: Page\n---\nSee [missing](/does/not/exist.md) and [also](../gone.md).\n",
    )
    .expect("broken links must not cause rejection");
    assert!(doc.body().contains("/does/not/exist.md"));
}

#[test]
fn all_recommended_fields_may_be_absent() {
    let doc = Document::parse("---\ntype: Page\n---\n").expect("recommended fields are optional");
    assert_eq!(doc.title(), None);
    assert_eq!(doc.description(), None);
    assert_eq!(doc.resource(), None);
    assert_eq!(doc.timestamp(), None);
    assert!(doc.tags().is_empty());
}

#[test]
fn title_falls_back_to_filename_stem() {
    let doc = Document::parse("---\ntype: Page\n---\n").unwrap();
    assert_eq!(doc.title_or_filename("kosong.md"), "kosong");
}

// ---------------------------------------------------------------------------
// Preservation: unknown keys
// ---------------------------------------------------------------------------

#[test]
fn unknown_top_level_keys_survive_a_round_trip() {
    let source = "\
---
type: Page
some_other_producer: keep me
nested_extension:
  inner: value
  count: 42
list_extension:
  - one
  - two
---
body
";
    let mut doc = Document::parse(source).unwrap();
    doc.set_title("Changed").unwrap();
    let output = doc.to_okf_string().unwrap();

    assert!(output.contains("some_other_producer: keep me"));
    assert!(output.contains("inner: value"));
    assert!(output.contains("count: 42"));
    assert!(output.contains("- one"));
    assert!(output.contains("- two"));

    // And they must still be there after a second cycle.
    let reparsed = Document::parse(&output).unwrap();
    assert!(
        reparsed
            .front_matter()
            .contains_key(serde_yaml_ng::Value::from("some_other_producer"))
    );
}

#[test]
fn unknown_keys_inside_the_kosong_block_survive_a_round_trip() {
    let source = "\
---
type: Page
kosong:
  profile: 1
  id: 01J7ZQ8F3K9XG2VW6M4T1B5NRY
  slug: my-page
  visibility: private
  created_at: 2026-07-26T10:30:00+08:00
  future_field: preserved
---
body
";
    let mut doc = Document::parse(source).unwrap();
    doc.touch();
    let output = doc.to_okf_string().unwrap();

    assert!(
        output.contains("future_field: preserved"),
        "unknown keys nested in `kosong` must round-trip:\n{output}"
    );
}

#[test]
fn front_matter_key_order_is_preserved() {
    // Keys are deliberately in reverse-alphabetical order, so a serializer
    // that sorts its output fails this test. Search terms include the colon
    // so they match the key and not a lookalike substring in a value.
    let source = "\
---
zebra: written first
type: Page
alpha: written last
---
body
";
    let doc = Document::parse(source).unwrap();
    let output = doc.to_okf_string().unwrap();

    let zebra = output.find("zebra:").expect("zebra present");
    let type_pos = output.find("type:").expect("type present");
    let alpha = output.find("alpha:").expect("alpha present");

    assert!(
        zebra < type_pos && type_pos < alpha,
        "key order must survive serialization:\n{output}"
    );
}

// ---------------------------------------------------------------------------
// Forward compatibility with OKF v0.2
// ---------------------------------------------------------------------------

#[test]
fn okf_v0_2_fields_round_trip_uninterpreted() {
    // v0.2 shipped 2026-07-24. v1 targets v0.1, but must never destroy a
    // v0.2 document it happens to open.
    let source = "\
---
type: Page
generated:
  by: reference_agent/gemini-2.5-pro
  at: 2026-07-24T10:00:00Z
verified:
  - by: human:ahormati
    at: 2026-07-25T09:00:00Z
status: draft
stale_after: 2026-12-31
sources:
  - resource: https://example.com/source
    id: src1
    title: Upstream source
---
body
";
    let mut doc = Document::parse(source).expect("v0.2 documents must parse");
    doc.set_title("Retitled").unwrap();
    let output = doc.to_okf_string().unwrap();

    for expected in [
        "reference_agent/gemini-2.5-pro",
        "human:ahormati",
        "status: draft",
        "stale_after: 2026-12-31",
        "https://example.com/source",
        "id: src1",
    ] {
        assert!(
            output.contains(expected),
            "v0.2 field `{expected}` was lost:\n{output}"
        );
    }
}

// ---------------------------------------------------------------------------
// Preservation: the body is byte-for-byte
// ---------------------------------------------------------------------------

/// Asserts a metadata-only change leaves the body byte-identical.
fn assert_body_preserved(source: &str, expected_body: &str) {
    let mut doc = Document::parse(source).expect("source must parse");
    assert_eq!(doc.body(), expected_body, "body wrong on read");

    doc.set_title("A completely different title").unwrap();
    doc.touch();
    let output = doc.to_okf_string().unwrap();

    let reparsed = Document::parse(&output).expect("output must re-parse");
    assert_eq!(
        reparsed.body(),
        expected_body,
        "body changed across a metadata-only write"
    );
}

#[test]
fn body_with_unicode_is_preserved() {
    assert_body_preserved(
        "---\ntype: Page\n---\n# Selamat datang 👋\n\nこんにちは — em-dash, ünïcödé.\n",
        "# Selamat datang 👋\n\nこんにちは — em-dash, ünïcödé.\n",
    );
}

#[test]
fn body_without_a_trailing_newline_is_preserved() {
    assert_body_preserved(
        "---\ntype: Page\n---\nno trailing newline",
        "no trailing newline",
    );
}

#[test]
fn empty_body_is_preserved() {
    assert_body_preserved("---\ntype: Page\n---\n", "");
}

#[test]
fn body_with_significant_trailing_whitespace_is_preserved() {
    // Two trailing spaces are a hard line break in Markdown, so this is
    // semantic content, not noise to be trimmed.
    assert_body_preserved(
        "---\ntype: Page\n---\nline one  \nline two\n\n\n",
        "line one  \nline two\n\n\n",
    );
}

#[test]
fn crlf_documents_keep_crlf_endings() {
    let source = "---\r\ntype: Page\r\ntitle: CRLF\r\n---\r\n# Windows\r\n\r\nBody line.\r\n";
    let mut doc = Document::parse(source).expect("CRLF source must parse");

    assert_eq!(doc.line_ending(), LineEnding::Crlf);
    assert_eq!(doc.body(), "# Windows\r\n\r\nBody line.\r\n");

    doc.touch();
    let output = doc.to_okf_string().unwrap();

    assert!(
        !output.contains("\n\n") || output.contains("\r\n"),
        "CRLF document must not be rewritten with bare LF delimiters"
    );
    assert!(output.starts_with("---\r\n"));
    assert_eq!(
        Document::parse(&output).unwrap().body(),
        "# Windows\r\n\r\nBody line.\r\n"
    );
}

#[test]
fn a_body_containing_a_delimiter_line_is_not_truncated() {
    // A `---` inside the body is a horizontal rule or a nested front matter
    // sample. Only the first closing delimiter ends the front matter.
    let source = "---\ntype: Page\n---\nIntro.\n\n---\n\nAfter a horizontal rule.\n";
    let doc = Document::parse(source).unwrap();
    assert_eq!(doc.body(), "Intro.\n\n---\n\nAfter a horizontal rule.\n");
}

#[test]
fn byte_order_mark_is_preserved() {
    let source = "\u{feff}---\ntype: Page\n---\nbody\n";
    let doc = Document::parse(source).expect("BOM-prefixed file must parse");
    assert_eq!(doc.doc_type(), "Page");
    assert!(doc.to_okf_string().unwrap().starts_with('\u{feff}'));
}

// ---------------------------------------------------------------------------
// Reserved filenames
// ---------------------------------------------------------------------------

#[test]
fn index_and_log_are_reserved() {
    assert!(okf::is_reserved_filename("index.md"));
    assert!(okf::is_reserved_filename("log.md"));

    // Case-insensitive, because macOS filesystems are.
    assert!(okf::is_reserved_filename("Index.md"));
    assert!(okf::is_reserved_filename("LOG.md"));

    assert!(!okf::is_reserved_filename("kosong.md"));
    assert!(!okf::is_reserved_filename("indexes.md"));
    assert!(!okf::is_reserved_filename("catalog.md"));
}

// ---------------------------------------------------------------------------
// Managed, unmanaged, and adoption
// ---------------------------------------------------------------------------

#[test]
fn a_document_without_a_kosong_block_is_unmanaged_not_invalid() {
    // This is the case that lets a user point kosong at a document produced
    // by some other OKF tool.
    let doc = Document::parse("---\ntype: BigQuery Table\ntitle: Orders\n---\n# Schema\n").unwrap();

    assert!(!doc.is_managed());
    assert_eq!(doc.kosong().unwrap(), None);
}

#[test]
fn adoption_preserves_the_body_byte_for_byte() {
    let body = "# Orders\n\n| Column | Type |\n|---|---|\n| `id` | STRING |\n";
    let source = format!("---\ntype: BigQuery Table\ntitle: Orders\n---\n{body}");

    let mut doc = Document::parse(&source).unwrap();
    let meta = doc.adopt("orders.md").unwrap();

    assert_eq!(meta.profile, okf::KOSONG_PROFILE);
    assert_eq!(meta.slug, "orders");
    assert_eq!(meta.visibility, Visibility::Private);
    assert!(doc.is_managed());
    assert_eq!(doc.body(), body, "adoption must not touch the body");

    // The original type is not overwritten with `Page`.
    assert_eq!(doc.doc_type(), "BigQuery Table");

    let reparsed = Document::parse(&doc.to_okf_string().unwrap()).unwrap();
    assert_eq!(reparsed.body(), body);
    assert_eq!(reparsed.kosong().unwrap().unwrap().id, meta.id);
}

#[test]
fn adoption_is_idempotent() {
    let mut doc = Document::parse("---\ntype: Page\n---\nbody\n").unwrap();
    let first = doc.adopt("kosong.md").unwrap();
    let second = doc.adopt("kosong.md").unwrap();
    assert_eq!(first, second, "adopting twice must not mint a new id");
}

// ---------------------------------------------------------------------------
// The kosong extension block
// ---------------------------------------------------------------------------

#[test]
fn a_valid_kosong_block_parses() {
    let source = "\
---
type: Page
title: My First Site
kosong:
  profile: 1
  id: 01J7ZQ8F3K9XG2VW6M4T1B5NRY
  slug: my-first-site
  visibility: public
  created_at: 2026-07-26T10:30:00+08:00
---
# My First Site
";
    let meta = Document::parse(source).unwrap().kosong().unwrap().unwrap();

    assert_eq!(meta.profile, 1);
    assert_eq!(meta.id, "01J7ZQ8F3K9XG2VW6M4T1B5NRY");
    assert_eq!(meta.slug, "my-first-site");
    assert_eq!(meta.visibility, Visibility::Public);
    assert_eq!(meta.created_at, "2026-07-26T10:30:00+08:00");
}

#[test]
fn an_unknown_kosong_profile_is_rejected() {
    let source = "\
---
type: Page
kosong:
  profile: 99
  id: 01J7ZQ8F3K9XG2VW6M4T1B5NRY
  slug: x
  visibility: private
  created_at: 2026-07-26T10:30:00+08:00
---
";
    let err = Document::parse(source).unwrap_err();
    assert!(matches!(err, OkfError::UnsupportedProfile { .. }));

    // This assertion used to require the repair to say `kosong update`, which
    // pinned a defect in place: no such command exists, so anyone following
    // the advice got `unrecognized subcommand`. A repair must name something
    // the reader can actually do.
    let repair = err.repair();
    assert!(
        repair.contains("get.kosong.dev"),
        "the repair must point at a real way to get a newer kosong: {repair}"
    );
    assert!(
        !repair.contains("kosong update"),
        "`kosong update` is not a command: {repair}"
    );
}

#[test]
fn a_kosong_block_missing_a_required_field_is_rejected() {
    let source = "---\ntype: Page\nkosong:\n  profile: 1\n  slug: x\n---\n";
    let err = Document::parse(source).unwrap_err();
    assert!(matches!(
        err,
        OkfError::KosongMissingField {
            field: "id" | "visibility" | "created_at"
        }
    ));
}

#[test]
fn an_invalid_visibility_is_rejected() {
    let source = "\
---
type: Page
kosong:
  profile: 1
  id: 01J7ZQ8F3K9XG2VW6M4T1B5NRY
  slug: x
  visibility: unlisted
  created_at: 2026-07-26T10:30:00+08:00
---
";
    let err = Document::parse(source).unwrap_err();
    assert!(matches!(
        err,
        OkfError::KosongInvalidField {
            field: "visibility",
            ..
        }
    ));
    assert!(err.repair().contains("private"));
}

#[test]
fn a_non_mapping_kosong_block_is_rejected() {
    let err = Document::parse("---\ntype: Page\nkosong: just a string\n---\n").unwrap_err();
    assert!(matches!(err, OkfError::KosongNotMapping));
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

#[test]
fn a_new_document_is_conformant_and_managed() {
    let doc = Document::new("My First Site").unwrap();
    let output = doc.to_okf_string().unwrap();

    let reparsed = Document::parse(&output).expect("a document we wrote must parse");
    assert_eq!(reparsed.doc_type(), DEFAULT_TYPE);
    assert_eq!(reparsed.title(), Some("My First Site"));
    assert_eq!(reparsed.tags(), vec!["kosong"]);
    assert!(reparsed.timestamp().is_some());

    let meta = reparsed.kosong().unwrap().unwrap();
    assert_eq!(meta.slug, "my-first-site");
    assert_eq!(meta.visibility, Visibility::Private);
    assert_eq!(meta.id.len(), 26, "ULIDs are 26 characters");
}

#[test]
fn an_over_long_title_is_rejected() {
    let long = "x".repeat(MAX_TITLE_CHARS + 1);
    assert!(matches!(Document::new(&long), Err(OkfError::TitleTooLong)));

    let mut doc = Document::new("ok").unwrap();
    assert!(matches!(doc.set_title(&long), Err(OkfError::TitleTooLong)));
}

#[test]
fn setters_touch_only_their_own_key() {
    let mut doc = Document::new("Original").unwrap();
    let original_id = doc.kosong().unwrap().unwrap().id;

    doc.set_description("New description.");
    doc.set_resource("https://my-first-site.pages.dev");

    assert_eq!(doc.title(), Some("Original"));
    assert_eq!(doc.description(), Some("New description."));
    assert_eq!(doc.resource(), Some("https://my-first-site.pages.dev"));
    assert_eq!(doc.kosong().unwrap().unwrap().id, original_id);
}

// ---------------------------------------------------------------------------
// Slugs
// ---------------------------------------------------------------------------

#[test]
fn slugify_produces_url_safe_output() {
    assert_eq!(okf::slugify("My First Site"), "my-first-site");
    assert_eq!(okf::slugify("  Leading & trailing  "), "leading-trailing");
    assert_eq!(okf::slugify("Hello -- World!!"), "hello-world");
    assert_eq!(okf::slugify("Version 2.0"), "version-2-0");
    assert_eq!(okf::slugify("ALLCAPS"), "allcaps");
    // Non-ASCII is dropped, so callers must handle an empty slug.
    assert_eq!(okf::slugify("日本語"), "");
    assert_eq!(okf::slugify(""), "");
}

// ---------------------------------------------------------------------------
// A realistic upstream document
// ---------------------------------------------------------------------------

#[test]
fn the_upstream_example_document_parses_unchanged() {
    // Adapted from the OKF announcement post. kosong must read documents it
    // did not write, from producers that know nothing about it.
    let source = "\
---
type: BigQuery Table
title: Orders
description: One row per completed customer order.
resource: https://console.cloud.google.com/bigquery?p=acme&d=sales&t=orders
tags: [sales, revenue]
timestamp: 2026-05-28T14:30:00Z
---
# Schema

| Column | Type | Description |
|--------|------|-------------|
| `order_id` | STRING | Globally unique order identifier. |
| `customer_id` | STRING | FK to [customers](/tables/customers.md). |
";
    let doc = Document::parse(source).expect("upstream sample must parse");

    assert_eq!(doc.doc_type(), "BigQuery Table");
    assert_eq!(doc.title(), Some("Orders"));
    assert_eq!(doc.tags(), vec!["sales", "revenue"]);
    assert_eq!(doc.timestamp(), Some("2026-05-28T14:30:00Z"));
    assert!(doc.resource().unwrap().starts_with("https://"));
    assert!(!doc.is_managed());

    // Round-tripping an untouched document must not lose anything.
    let reparsed = Document::parse(&doc.to_okf_string().unwrap()).unwrap();
    assert_eq!(reparsed.body(), doc.body());
    assert_eq!(reparsed.tags(), vec!["sales", "revenue"]);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `Result::unwrap_err` with a custom message, which the std method lacks
/// unless the `Ok` type is `Debug`.
trait UnwrapErrOrPanic<T, E> {
    fn unwrap_err_or_panic(self, message: &str) -> E;
}

impl<T, E> UnwrapErrOrPanic<T, E> for Result<T, E> {
    fn unwrap_err_or_panic(self, message: &str) -> E {
        match self {
            Ok(_) => panic!("{message}"),
            Err(e) => e,
        }
    }
}
