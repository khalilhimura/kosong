//! `kosong show` — display the page.
//!
//! Shows metadata and content. There are no secrets in an OKF document, so
//! everything in it is safe to print; the care needed here is about being
//! readable, not about redaction.

use super::Context;
use crate::exit::CliResult;
use crate::exit::CliError;
use kosong_core::workspace::read_checked;

/// Returns the raw document text without printing it.
///
/// Reads the workspace document byte-for-byte from disk and trims trailing
/// whitespace, just like `show --raw`, but returns the string instead of
/// printing it.
pub fn show_raw(context: &Context) -> CliResult<String> {
    let workspace = context.workspace()?;
    let path = workspace.document_path();
    read_checked(path)
        .map(|s| s.trim_end().to_owned())
        .map_err(|e| {
            CliError::internal(
                "SHOW_RAW_FAILED",
                format!("could not read document: {e}"),
            )
            .with_repair(e.repair())
        })
}

pub fn run(context: &Context, raw: bool) -> CliResult<()> {
    let ui = context.ui;
    let workspace = context.workspace()?;

    if raw {
        // Exactly what is on disk, byte for byte. Survives --quiet because it
        // is the thing the user explicitly asked to see.
        ui.always(read_checked(workspace.document_path())?.trim_end());
        return Ok(());
    }

    let document = workspace.read_document()?;
    let filename = workspace.document_path().file_name().unwrap_or("kosong.md");

    ui.heading(document.title_or_filename(filename));
    ui.blank();
    ui.field("file", workspace.document_path().as_str());
    ui.field("type", document.doc_type());

    if let Some(description) = document.description() {
        ui.field("description", description);
    }
    if let Some(timestamp) = document.timestamp() {
        ui.field("changed", timestamp);
    }
    if let Some(resource) = document.resource() {
        ui.field("published", resource);
    }

    let tags = document.tags();
    if !tags.is_empty() {
        ui.field("tags", tags.join(", "));
    }

    match document.kosong()? {
        Some(meta) => {
            ui.field("id", &meta.id);
            ui.field("slug", &meta.slug);
            ui.field("visibility", meta.visibility.as_str());
        }
        None => {
            ui.blank();
            ui.say(
                "This file follows the Open Knowledge Format but was not made by kosong.\n\
                 Run `kosong status` to let kosong look after it.",
            );
        }
    }

    ui.blank();
    ui.always(document.body().trim_end());
    Ok(())
}
