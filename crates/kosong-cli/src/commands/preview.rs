//! `kosong preview` — serve the page locally.
//!
//! Lesson: a local server is a temporary website on your own computer.

use super::Context;
use crate::exit::CliResult;
use kosong_core::{Preview, render};

pub fn run(context: &Context, port: Option<u16>) -> CliResult<()> {
    let ui = context.ui;
    let workspace = context.workspace()?;
    let document = workspace.read_document()?;

    let filename = workspace.document_path().file_name().unwrap_or("kosong.md");
    let rendered = render::render_page(&document, filename);

    let port = port.unwrap_or_else(|| {
        context
            .config()
            .map(|c| c.preview_port())
            .unwrap_or(kosong_core::preview::DEFAULT_PORT)
    });

    let server = Preview::for_page(port, &rendered)?;

    // Say what was changed before serving. Silently altering someone's page
    // would undercut the point of showing them how this works.
    if let Some(note) = rendered.removal_note() {
        ui.warn(note);
        ui.blank();
    }

    ui.heading(format!("Serving your page at {}", server.url()));
    ui.blank();
    ui.lesson(
        "This address only works on this computer. Nobody else can reach it.\n\
         Press Ctrl-C to stop.",
    );

    // Progress is recorded once the address exists, because that is the point
    // at which the user has actually seen a working preview.
    let mut onboarding = context.onboarding()?;
    if !onboarding.preview_completed {
        onboarding.preview_completed = true;
        context.save_onboarding(&onboarding);
    }

    server.serve_forever()?;
    Ok(())
}
