//! Open Knowledge Format v0.1 documents.
//!
//! `kosong` does not define a file format. It profiles Google Cloud's Open
//! Knowledge Format, an open specification for representing knowledge as a
//! directory of Markdown files with YAML front matter.
//!
//! - Specification: <https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md>
//! - Profile: `spec/okf-profile-v1.md`
//!
//! Two upstream rules drive the whole design of this module:
//!
//! 1. **`type` is the only required field.** A document carrying nothing but
//!    `type` is conformant and must parse.
//! 2. **Unknown fields must be preserved.** A consumer may not drop keys it
//!    does not recognise, because another producer may depend on them.
//!
//! Rule 2 is why front matter is held as a [`Mapping`] rather than a typed
//! struct: `serde` deserialization into a fixed struct silently discards
//! unknown keys on write, which would corrupt every document `kosong` touches.
//! Typed access is layered on top as accessors that mutate single keys in
//! place.
//!
//! The Markdown body is captured as the exact text following the closing
//! delimiter and is never re-parsed or re-emitted, which is what makes the
//! byte-for-byte body guarantee hold.

use serde_yaml_ng::{Mapping, Value};
use std::fmt;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The OKF specification version this implementation targets.
pub const OKF_VERSION: &str = "0.1";

/// Version of the `kosong` front-matter extension block.
///
/// This versions the extension only. It is independent of [`OKF_VERSION`] and
/// of the remote API version.
pub const KOSONG_PROFILE: u64 = 1;

/// The `type` value `kosong` writes. Any other value is read without complaint.
pub const DEFAULT_TYPE: &str = "Page";

/// The default document filename.
pub const DEFAULT_FILENAME: &str = "kosong.md";

/// Filenames OKF reserves. These are never concept documents.
pub const RESERVED_FILENAMES: [&str; 2] = ["index.md", "log.md"];

/// Front-matter key holding the `kosong` extension block.
const KOSONG_KEY: &str = "kosong";

/// Maximum accepted length of `title`, in characters.
pub const MAX_TITLE_CHARS: usize = 200;

/// Returns whether `name` is a filename OKF reserves for a non-concept role.
///
/// Comparison is case-insensitive: on macOS's default case-insensitive
/// filesystem, `Index.md` and `index.md` are the same file, so treating them
/// differently would let a document be created that shadows a reserved name.
pub fn is_reserved_filename(name: &str) -> bool {
    RESERVED_FILENAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(name))
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A document that could not be read as conformant OKF.
///
/// Every variant can produce a [repair suggestion](OkfError::repair), because
/// the product requires that a validation failure always tells the user what
/// to do next.
#[derive(Debug, thiserror::Error)]
pub enum OkfError {
    #[error("this file has no front matter")]
    MissingFrontMatter,

    #[error("the front matter is never closed")]
    UnterminatedFrontMatter,

    #[error("the front matter is not valid YAML: {0}")]
    InvalidYaml(#[from] serde_yaml_ng::Error),

    #[error("the front matter is not a set of key/value pairs")]
    FrontMatterNotMapping,

    #[error("the required `type` field is missing")]
    MissingType,

    #[error("the `type` field is empty")]
    EmptyType,

    #[error("`title` is longer than {MAX_TITLE_CHARS} characters")]
    TitleTooLong,

    #[error("the `kosong` block is not a set of key/value pairs")]
    KosongNotMapping,

    #[error(
        "this file uses kosong profile {found}, but this version understands profile {KOSONG_PROFILE}"
    )]
    UnsupportedProfile { found: String },

    #[error("the `kosong` block is missing `{field}`")]
    KosongMissingField { field: &'static str },

    #[error("the `kosong` block field `{field}` should be {expected}")]
    KosongInvalidField {
        field: &'static str,
        expected: &'static str,
    },

    #[error("`{name}` is a filename the Open Knowledge Format reserves")]
    ReservedFilename { name: String },
}

impl OkfError {
    /// A plain-language next action, in the CLI's voice.
    pub fn repair(&self) -> String {
        match self {
            Self::MissingFrontMatter => format!(
                "Add a block like this to the very top of the file:\n\n---\ntype: {DEFAULT_TYPE}\ntitle: \"My First Site\"\n---"
            ),
            Self::UnterminatedFrontMatter => {
                "The block that starts with --- needs a matching --- on its own line to close it.".into()
            }
            Self::InvalidYaml(_) => {
                "Check the front matter for a stray quote, tab, or missing colon. Values with a colon in them need quotes.".into()
            }
            Self::FrontMatterNotMapping => {
                "The front matter needs `key: value` lines, not a list or a bare value.".into()
            }
            Self::MissingType => format!(
                "Add `type: {DEFAULT_TYPE}` to the front matter. It is the one field the Open Knowledge Format requires."
            ),
            Self::EmptyType => format!("Give `type` a value, for example `type: {DEFAULT_TYPE}`."),
            Self::TitleTooLong => {
                format!("Shorten `title` to {MAX_TITLE_CHARS} characters or fewer.")
            }
            Self::KosongNotMapping => {
                "The `kosong:` block needs indented `key: value` lines beneath it.".into()
            }
            Self::UnsupportedProfile { .. } => {
                // Not "run kosong update": there is no such command, and a
                // repair that points at something that does not exist is worse
                // than no repair at all.
                "This file was made by a newer kosong. Install the current version:\n  \
                 curl -fsSL https://get.kosong.dev | sh"
                    .into()
            }
            Self::KosongMissingField { field } => {
                format!("Add `{field}` inside the `kosong:` block, or delete the whole block and run: kosong status")
            }
            Self::KosongInvalidField { field, expected } => {
                format!("Set `{field}` to {expected} inside the `kosong:` block.")
            }
            Self::ReservedFilename { .. } => format!(
                "The Open Knowledge Format uses index.md and log.md for other purposes. Use a different name, such as {DEFAULT_FILENAME}."
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Line endings
// ---------------------------------------------------------------------------

/// The line ending a document uses, so writes match what was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    Crlf,
}

impl LineEnding {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }
}

// ---------------------------------------------------------------------------
// Visibility
// ---------------------------------------------------------------------------

/// Whether the published page is meant to be listed publicly.
///
/// This affects the site template and deployment only. It is **never** the
/// basis for remote object authorization, which derives solely from the
/// authenticated session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    #[default]
    Private,
    Public,
}

impl Visibility {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Public => "public",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "private" => Some(Self::Private),
            "public" => Some(Self::Public),
            _ => None,
        }
    }
}

impl fmt::Display for Visibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// The kosong extension block
// ---------------------------------------------------------------------------

/// A typed view of the `kosong` front-matter extension block.
///
/// This is a *view*: reading it does not detach it from the document, and
/// unknown keys nested inside `kosong` are preserved in the underlying mapping
/// regardless of what this struct models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KosongMeta {
    pub profile: u64,
    pub id: String,
    pub slug: String,
    pub visibility: Visibility,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

/// A parsed OKF concept document.
///
/// Holds front matter as an order-preserving mapping and the body as untouched
/// text. See the module docs for why.
#[derive(Debug, Clone)]
pub struct Document {
    front: Mapping,
    body: String,
    line_ending: LineEnding,
    byte_order_mark: bool,
}

impl Document {
    /// Parses OKF text, validating only what OKF and the `kosong` profile
    /// actually require.
    ///
    /// Unknown `type` values, absent recommended fields, unknown keys, and
    /// broken links are all accepted: OKF forbids consumers from rejecting
    /// documents for any of them.
    pub fn parse(input: &str) -> Result<Self, OkfError> {
        let (byte_order_mark, text) = match input.strip_prefix('\u{feff}') {
            Some(rest) => (true, rest),
            None => (false, input),
        };

        let (line_ending, yaml, body) = split_front_matter(text)?;

        // An empty front-matter block parses as YAML null, not a mapping.
        let front = match serde_yaml_ng::from_str::<Value>(yaml)? {
            Value::Mapping(map) => map,
            Value::Null => return Err(OkfError::MissingType),
            _ => return Err(OkfError::FrontMatterNotMapping),
        };

        let doc = Self {
            front,
            body: body.to_owned(),
            line_ending,
            byte_order_mark,
        };
        doc.validate()?;
        Ok(doc)
    }

    /// Checks the conformance rules. Called by [`parse`](Self::parse).
    fn validate(&self) -> Result<(), OkfError> {
        match self.front.get(Value::from("type")) {
            None => return Err(OkfError::MissingType),
            Some(Value::String(s)) if s.trim().is_empty() => return Err(OkfError::EmptyType),
            Some(Value::String(_)) => {}
            // A bare `type:` with no value parses as YAML null. That is an
            // absent value, not an exotic one, so it fails the "non-empty
            // `type`" conformance criterion.
            Some(Value::Null) => return Err(OkfError::EmptyType),
            // Any other non-string `type` is a producer quirk, not grounds for
            // rejection; it is preserved and surfaced via `type_raw`.
            Some(_) => {}
        }

        if let Some(title) = self.title() {
            if title.chars().count() > MAX_TITLE_CHARS {
                return Err(OkfError::TitleTooLong);
            }
        }

        // Absent is fine: an unmanaged document is still valid OKF.
        if self.front.contains_key(Value::from(KOSONG_KEY)) {
            self.kosong()?;
        }

        Ok(())
    }

    // -- OKF core fields ---------------------------------------------------

    /// The required `type` field, or `""` if it is not a plain string.
    pub fn doc_type(&self) -> &str {
        self.str_field("type").unwrap_or_default()
    }

    /// The raw `type` value, for the rare producer that writes a non-string.
    pub fn type_raw(&self) -> Option<&Value> {
        self.front.get(Value::from("type"))
    }

    /// The recommended `title`. OKF permits consumers to fall back to the
    /// filename stem when absent; see [`title_or_filename`](Self::title_or_filename).
    pub fn title(&self) -> Option<&str> {
        self.str_field("title")
    }

    /// `title`, falling back to the filename stem as OKF allows.
    pub fn title_or_filename(&self, filename: &str) -> String {
        self.title()
            .map(str::to_owned)
            .unwrap_or_else(|| filename.trim_end_matches(".md").to_owned())
    }

    pub fn description(&self) -> Option<&str> {
        self.str_field("description")
    }

    /// The `resource` URI, set to the deployment URL after a successful publish.
    pub fn resource(&self) -> Option<&str> {
        self.str_field("resource")
    }

    /// `tags`, skipping any non-string entries rather than failing.
    pub fn tags(&self) -> Vec<&str> {
        match self.front.get(Value::from("tags")) {
            Some(Value::Sequence(seq)) => seq.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        }
    }

    /// The `timestamp` of the last meaningful change, RFC 3339.
    ///
    /// This is OKF v0.1's field for the concept earlier `kosong` drafts called
    /// `updated_at`. There is no separate `updated_at`.
    pub fn timestamp(&self) -> Option<&str> {
        self.str_field("timestamp")
    }

    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    /// The whole front matter, for callers that need a key this API does not model.
    pub fn front_matter(&self) -> &Mapping {
        &self.front
    }

    // -- The kosong extension block ---------------------------------------

    /// Whether the document carries a `kosong` block, i.e. is *managed*.
    pub fn is_managed(&self) -> bool {
        self.front.contains_key(Value::from(KOSONG_KEY))
    }

    /// Reads and validates the `kosong` block.
    ///
    /// Returns `Ok(None)` for an unmanaged document, which is a valid state,
    /// not an error.
    pub fn kosong(&self) -> Result<Option<KosongMeta>, OkfError> {
        let Some(value) = self.front.get(Value::from(KOSONG_KEY)) else {
            return Ok(None);
        };
        let Value::Mapping(map) = value else {
            return Err(OkfError::KosongNotMapping);
        };

        let profile = match map.get(Value::from("profile")) {
            None => {
                return Err(OkfError::KosongMissingField { field: "profile" });
            }
            Some(Value::Number(n)) => n.as_u64().ok_or(OkfError::KosongInvalidField {
                field: "profile",
                expected: "a whole number",
            })?,
            Some(other) => {
                return Err(OkfError::UnsupportedProfile {
                    found: describe(other),
                });
            }
        };
        if profile != KOSONG_PROFILE {
            return Err(OkfError::UnsupportedProfile {
                found: profile.to_string(),
            });
        }

        let required = |field: &'static str| -> Result<String, OkfError> {
            match map.get(Value::from(field)) {
                None => Err(OkfError::KosongMissingField { field }),
                Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
                Some(_) => Err(OkfError::KosongInvalidField {
                    field,
                    expected: "a non-empty piece of text",
                }),
            }
        };

        let id = required("id")?;
        let slug = required("slug")?;
        let created_at = required("created_at")?;
        let visibility_raw = required("visibility")?;
        let visibility =
            Visibility::parse(&visibility_raw).ok_or(OkfError::KosongInvalidField {
                field: "visibility",
                expected: "either `private` or `public`",
            })?;

        Ok(Some(KosongMeta {
            profile,
            id,
            slug,
            visibility,
            created_at,
        }))
    }

    // -- Mutation ----------------------------------------------------------
    //
    // Each setter touches exactly one key, leaving every other key — known,
    // unknown, or from a future OKF version — untouched and in place.

    pub fn set_title(&mut self, title: &str) -> Result<(), OkfError> {
        if title.chars().count() > MAX_TITLE_CHARS {
            return Err(OkfError::TitleTooLong);
        }
        self.front
            .insert(Value::from("title"), Value::from(title.trim()));
        Ok(())
    }

    pub fn set_description(&mut self, description: &str) {
        self.front
            .insert(Value::from("description"), Value::from(description.trim()));
    }

    /// Sets `resource`, the URI of the underlying asset. Called after publish.
    pub fn set_resource(&mut self, uri: &str) {
        self.front
            .insert(Value::from("resource"), Value::from(uri.trim()));
    }

    /// Sets `timestamp` to now, recording a meaningful change.
    pub fn touch(&mut self) {
        self.set_timestamp(&now_rfc3339());
    }

    pub fn set_timestamp(&mut self, rfc3339: &str) {
        self.front
            .insert(Value::from("timestamp"), Value::from(rfc3339));
    }

    /// Replaces the Markdown body.
    pub fn set_body(&mut self, body: &str) {
        self.body = body.to_owned();
    }

    /// Writes the `kosong` block, preserving any unknown keys already nested
    /// inside it.
    pub fn set_kosong(&mut self, meta: &KosongMeta) {
        let mut map = match self.front.get(Value::from(KOSONG_KEY)) {
            Some(Value::Mapping(existing)) => existing.clone(),
            _ => Mapping::new(),
        };
        map.insert(Value::from("profile"), Value::from(meta.profile));
        map.insert(Value::from("id"), Value::from(meta.id.as_str()));
        map.insert(Value::from("slug"), Value::from(meta.slug.as_str()));
        map.insert(
            Value::from("visibility"),
            Value::from(meta.visibility.as_str()),
        );
        map.insert(
            Value::from("created_at"),
            Value::from(meta.created_at.as_str()),
        );
        self.front
            .insert(Value::from(KOSONG_KEY), Value::Mapping(map));
    }

    /// Adopts an unmanaged document by writing a `kosong` block.
    ///
    /// The body is untouched. This is what lets `kosong` take over a document
    /// produced by any other OKF tool.
    pub fn adopt(&mut self, filename: &str) -> Result<KosongMeta, OkfError> {
        if let Some(existing) = self.kosong()? {
            return Ok(existing);
        }
        let now = now_rfc3339();
        let title = self.title_or_filename(filename);
        let meta = KosongMeta {
            profile: KOSONG_PROFILE,
            id: ulid::Ulid::new().to_string(),
            slug: slugify(&title),
            visibility: Visibility::Private,
            created_at: now.clone(),
        };
        self.set_kosong(&meta);
        self.set_timestamp(&now);
        Ok(meta)
    }

    // -- Construction ------------------------------------------------------

    /// Builds a new managed document.
    pub fn new(title: &str) -> Result<Self, OkfError> {
        let title = title.trim();
        if title.chars().count() > MAX_TITLE_CHARS {
            return Err(OkfError::TitleTooLong);
        }
        let now = now_rfc3339();
        let slug = slugify(title);

        let mut front = Mapping::new();
        front.insert(Value::from("type"), Value::from(DEFAULT_TYPE));
        front.insert(Value::from("title"), Value::from(title));
        front.insert(
            Value::from("description"),
            Value::from("A page published with kosong."),
        );
        front.insert(
            Value::from("tags"),
            Value::Sequence(vec![Value::from("kosong")]),
        );
        front.insert(Value::from("timestamp"), Value::from(now.as_str()));

        let mut doc = Self {
            front,
            body: format!("# {title}\n\nStart here.\n"),
            line_ending: LineEnding::default(),
            byte_order_mark: false,
        };
        doc.set_kosong(&KosongMeta {
            profile: KOSONG_PROFILE,
            id: ulid::Ulid::new().to_string(),
            slug,
            visibility: Visibility::Private,
            created_at: now,
        });
        Ok(doc)
    }

    // -- Serialization -----------------------------------------------------

    /// Renders the document back to OKF text.
    ///
    /// The body is emitted verbatim. Front-matter *formatting* — quoting,
    /// indentation, comments — is not preserved, because the YAML is
    /// re-serialized; key order and content are. This limitation is recorded
    /// in `spec/okf-profile-v1.md` §4.3.
    pub fn to_okf_string(&self) -> Result<String, OkfError> {
        let yaml = serde_yaml_ng::to_string(&Value::Mapping(self.front.clone()))?;
        let nl = self.line_ending.as_str();

        let mut out = String::with_capacity(yaml.len() + self.body.len() + 16);
        if self.byte_order_mark {
            out.push('\u{feff}');
        }
        out.push_str("---");
        out.push_str(nl);
        for line in yaml.lines() {
            out.push_str(line);
            out.push_str(nl);
        }
        out.push_str("---");
        out.push_str(nl);
        out.push_str(&self.body);
        Ok(out)
    }

    // -- Internals ---------------------------------------------------------

    fn str_field(&self, key: &str) -> Option<&str> {
        self.front.get(Value::from(key))?.as_str()
    }
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

/// Splits `---` delimited front matter from the body.
///
/// Returns the detected line ending, the YAML text, and the body. The body is
/// borrowed from `text` and never copied or normalized, which is what upholds
/// the byte-for-byte guarantee.
fn split_front_matter(text: &str) -> Result<(LineEnding, &str, &str), OkfError> {
    let (line_ending, after_open) = if let Some(rest) = text.strip_prefix("---\r\n") {
        (LineEnding::Crlf, rest)
    } else if let Some(rest) = text.strip_prefix("---\n") {
        (LineEnding::Lf, rest)
    } else if text.trim_end() == "---" {
        return Err(OkfError::UnterminatedFrontMatter);
    } else {
        return Err(OkfError::MissingFrontMatter);
    };

    let mut offset = 0usize;
    loop {
        let remaining = &after_open[offset..];
        if remaining.is_empty() {
            return Err(OkfError::UnterminatedFrontMatter);
        }
        let line_len = remaining
            .find('\n')
            .map_or(remaining.len(), |index| index + 1);
        let line = &remaining[..line_len];

        if matches!(line.trim_end_matches(['\n', '\r']), "---" | "...") {
            return Ok((line_ending, &after_open[..offset], &remaining[line_len..]));
        }
        offset += line_len;
    }
}

/// Converts a title into a lowercase, URL-safe slug.
///
/// Non-alphanumeric ASCII collapses to single hyphens. Non-ASCII characters are
/// dropped, so a title with no ASCII alphanumerics yields an empty slug; callers
/// substitute a fallback.
pub fn slugify(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut pending_separator = false;

    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_separator && !slug.is_empty() {
                slug.push('-');
            }
            pending_separator = false;
            slug.push(ch.to_ascii_lowercase());
        } else {
            pending_separator = true;
        }
    }
    slug
}

/// The current time as an RFC 3339 string, in local offset where available.
///
/// Falls back to UTC when the offset cannot be determined, which happens in
/// multi-threaded processes on some Unix platforms.
/// Truncated to whole seconds. Sub-second precision is noise in a file a
/// person is meant to read, and nothing in `kosong` orders events finely
/// enough to need it.
pub fn now_rfc3339() -> String {
    OffsetDateTime::now_local()
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .replace_nanosecond(0)
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .format(&Rfc3339)
        .unwrap_or_default()
}

/// Names a YAML value's kind, for error messages that must not leak content.
fn describe(value: &Value) -> String {
    match value {
        Value::Null => "nothing".into(),
        Value::Bool(_) => "true/false".into(),
        Value::Number(n) => n.to_string(),
        Value::String(_) => "text".into(),
        Value::Sequence(_) => "a list".into(),
        Value::Mapping(_) => "a block".into(),
        Value::Tagged(_) => "a tagged value".into(),
    }
}
