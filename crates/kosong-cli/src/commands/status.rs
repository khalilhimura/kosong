//! `kosong status` — report the current state.
//!
//! Lesson: software can describe its current state.
//!
//! The `--json` shape is a stable contract. §17 names `status --json` as the
//! first step toward a read-only multi-harness interface, so fields may be
//! added but not renamed or removed without a schema bump.
//!
//! Nothing here may expose a secret: no tokens, no verification codes, no
//! email address. The document itself contains none, and session details are
//! reduced to a boolean.

use super::Context;
use crate::exit::{CliError, CliResult};
use crate::ui::Mark;
use kosong_core::okf::{self, Document};
use serde::Serialize;

/// Version of the `--json` shape.
const SCHEMA: u32 = 1;

#[derive(Debug, Serialize)]
struct StatusJson {
    schema: u32,
    okf_version: &'static str,
    workspace: WorkspaceJson,
    document: DocumentJson,
    onboarding: OnboardingJson,
    session: SessionJson,
}

#[derive(Debug, Serialize)]
struct WorkspaceJson {
    found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

#[derive(Debug, Serialize)]
struct DocumentJson {
    exists: bool,
    valid: bool,
    /// Whether the document carries a `kosong` block.
    managed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource: Option<String>,
}

#[derive(Debug, Serialize)]
struct OnboardingJson {
    local_document_created: bool,
    preview_completed: bool,
    login_completed: bool,
    site_initialized: bool,
    site_published: bool,
    next_command: String,
}

/// Session state. Never carries a token, a code, or an email address.
#[derive(Debug, Serialize)]
struct SessionJson {
    signed_in: bool,
    /// Where the credential is kept, so a file fallback is visible.
    #[serde(skip_serializing_if = "Option::is_none")]
    stored_in: Option<String>,
}

/// Reports whether a credential exists locally, without any network call.
///
/// `status` must stay under a second and must work offline, so this checks
/// only for a stored refresh token. Whether the server still honours it is a
/// question for the command that actually needs it.
fn session_state() -> SessionJson {
    match kosong_core::session::open_store() {
        Ok(store) => {
            let signed_in = matches!(store.load(), Ok(Some(_)));
            SessionJson {
                signed_in,
                stored_in: signed_in.then(|| store.backend().describe().to_owned()),
            }
        }
        Err(_) => SessionJson {
            signed_in: false,
            stored_in: None,
        },
    }
}

/// Returns the `status --json` output as a JSON string, or an error.
/// Does not print anything.
pub fn status_json(context: &Context) -> CliResult<String> {
    let workspace = context.workspace().ok();
    let onboarding = context.onboarding()?;

    let (document_json, _parsed) = match &workspace {
        Some(workspace) if workspace.has_document() => {
            let path = workspace.document_path().to_string();
            match workspace.read_document() {
                Ok(document) => (describe(&document, path)?, Some(document)),
                Err(error) => (
                    DocumentJson {
                        exists: true,
                        valid: false,
                        managed: false,
                        path: Some(path),
                        error: Some(error.to_string()),
                        r#type: None,
                        title: None,
                        slug: None,
                        visibility: None,
                        id: None,
                        timestamp: None,
                        resource: None,
                    },
                    None,
                ),
            }
        }
        _ => (
            DocumentJson {
                exists: false,
                valid: false,
                managed: false,
                path: workspace.as_ref().map(|w| w.document_path().to_string()),
                error: None,
                r#type: None,
                title: None,
                slug: None,
                visibility: None,
                id: None,
                timestamp: None,
                resource: None,
            },
            None,
        ),
    };

    let report = StatusJson {
        schema: SCHEMA,
        okf_version: okf::OKF_VERSION,
        workspace: WorkspaceJson {
            found: workspace.is_some(),
            path: workspace.as_ref().map(|w| w.root().to_string()),
        },
        document: document_json,
        onboarding: OnboardingJson {
            local_document_created: onboarding.local_document_created,
            preview_completed: onboarding.preview_completed,
            login_completed: onboarding.login_completed,
            site_initialized: onboarding.site_initialized,
            site_published: onboarding.site_published,
            next_command: onboarding.next_step().command().to_owned(),
        },
        session: session_state(),
    };

    serde_json::to_string_pretty(&report).map_err(|e| {
        CliError::internal("JSON_FAILED", format!("could not build JSON output: {e}"))
    })
}

pub fn run(context: &Context, json: bool) -> CliResult<()> {
    let ui = context.ui;

    let workspace = context.workspace().ok();
    let onboarding = context.onboarding()?;

    let (document_json, parsed) = match &workspace {
        Some(workspace) if workspace.has_document() => {
            let path = workspace.document_path().to_string();
            match workspace.read_document() {
                Ok(document) => (describe(&document, path)?, Some(document)),
                Err(error) => (
                    DocumentJson {
                        exists: true,
                        valid: false,
                        managed: false,
                        path: Some(path),
                        error: Some(error.to_string()),
                        r#type: None,
                        title: None,
                        slug: None,
                        visibility: None,
                        id: None,
                        timestamp: None,
                        resource: None,
                    },
                    None,
                ),
            }
        }
        _ => (
            DocumentJson {
                exists: false,
                valid: false,
                managed: false,
                path: workspace.as_ref().map(|w| w.document_path().to_string()),
                error: None,
                r#type: None,
                title: None,
                slug: None,
                visibility: None,
                id: None,
                timestamp: None,
                resource: None,
            },
            None,
        ),
    };

    let invalid = document_json.exists && !document_json.valid;

    let report = StatusJson {
        schema: SCHEMA,
        okf_version: okf::OKF_VERSION,
        workspace: WorkspaceJson {
            found: workspace.is_some(),
            path: workspace.as_ref().map(|w| w.root().to_string()),
        },
        document: document_json,
        onboarding: OnboardingJson {
            local_document_created: onboarding.local_document_created,
            preview_completed: onboarding.preview_completed,
            login_completed: onboarding.login_completed,
            site_initialized: onboarding.site_initialized,
            site_published: onboarding.site_published,
            next_command: onboarding.next_step().command().to_owned(),
        },
        session: session_state(),
    };

    if json {
        let text = status_json(context)?;
        ui.always(text);
    } else {
        print_human(context, &report, parsed.as_ref());
    }

    if invalid {
        // Reported fully, then flagged as an unmet local precondition so
        // scripts can detect it.
        return Err(CliError::usage(
            "INVALID_DOCUMENT",
            "your page has a problem that needs fixing",
        )
        .with_repair("Run `kosong doctor` for the details."));
    }
    Ok(())
}

fn describe(document: &Document, path: String) -> CliResult<DocumentJson> {
    let meta = document.kosong()?;
    Ok(DocumentJson {
        exists: true,
        valid: true,
        managed: meta.is_some(),
        path: Some(path),
        error: None,
        r#type: Some(document.doc_type().to_owned()),
        title: document.title().map(str::to_owned),
        slug: meta.as_ref().map(|m| m.slug.clone()),
        visibility: meta.as_ref().map(|m| m.visibility.as_str().to_owned()),
        id: meta.as_ref().map(|m| m.id.clone()),
        timestamp: document.timestamp().map(str::to_owned),
        resource: document.resource().map(str::to_owned),
    })
}

fn print_human(context: &Context, report: &StatusJson, document: Option<&Document>) {
    let ui = context.ui;

    match (&report.workspace.path, report.document.exists) {
        (None, _) => {
            ui.status(Mark::Info, "no page yet", "");
            ui.blank();
            ui.say("kosong has not made a page in this folder.");
            ui.next_command("kosong start");
            return;
        }
        (Some(path), false) => {
            ui.status(Mark::Info, "no page yet", path);
            ui.next_command("kosong new");
            return;
        }
        _ => {}
    }

    if !report.document.valid {
        // No next command here: `run` returns an error whose repair action
        // says to run `doctor`, and printing it twice reads as a stutter.
        ui.status(
            Mark::Bad,
            "your page needs fixing",
            report.document.error.as_deref().unwrap_or(""),
        );
        if let Some(path) = &report.document.path {
            ui.field("file", path);
        }
        return;
    }

    let title = report.document.title.as_deref().unwrap_or("(no title)");
    ui.status(Mark::Good, "your page is fine", title);
    ui.blank();

    if let Some(path) = &report.document.path {
        ui.field("file", path);
    }
    ui.field("type", report.document.r#type.as_deref().unwrap_or("—"));
    if let Some(slug) = &report.document.slug {
        ui.field("slug", slug);
    }
    if let Some(visibility) = &report.document.visibility {
        ui.field("visibility", visibility);
    }
    if let Some(resource) = &report.document.resource {
        ui.field("published", resource);
    }

    if !report.document.managed && document.is_some() {
        ui.blank();
        ui.status(
            Mark::Info,
            "not managed by kosong",
            "it follows the Open Knowledge Format, but has no kosong details",
        );
        ui.say("kosong can look after it without changing anything you wrote.");
        offer_adoption(context);
    }

    ui.blank();
    ui.field(
        "signed in",
        if report.session.signed_in {
            "yes"
        } else {
            "no"
        },
    );
    ui.next_command(&report.onboarding.next_command);
}

/// Offers to write a `kosong` block into an unmanaged document.
///
/// Declining is fine and leaves the file untouched, which is the point: the
/// document belongs to the user, not to `kosong`.
fn offer_adoption(context: &Context) {
    let ui = context.ui;
    if ui.is_quiet() {
        return;
    }

    if !ui.confirm("Add kosong's details to this file?", false) {
        return;
    }

    let result = (|| -> CliResult<String> {
        let workspace = context.workspace()?;
        let mut document = workspace.read_document()?;
        let filename = workspace
            .document_path()
            .file_name()
            .unwrap_or("kosong.md")
            .to_owned();
        let meta = document.adopt(&filename)?;
        workspace.write_document(&document)?;
        Ok(meta.id)
    })();

    match result {
        Ok(id) => {
            ui.status(Mark::Good, "done", format!("id {id}"));
            ui.lesson("Your text was not changed. Only kosong's own details were added.");
        }
        Err(error) => ui.error(&error.message, error.repair.as_deref()),
    }
}
