//! `kosong start` — resumable onboarding.
//!
//! The first stage is entirely local. §7.2 requires `start` to say so
//! explicitly: no account, no network, nothing to sign up for. That ordering
//! is the product's core promise, so this command never asks for an email.
//!
//! Being resumable means it is safe to run repeatedly. It looks at what
//! already exists, does the next missing thing, and stops.

use super::Context;
use crate::exit::CliResult;
use crate::ui::Mark;
use kosong_core::okf::Document;

pub fn run(context: &Context) -> CliResult<()> {
    let ui = context.ui;

    ui.heading("Welcome to kosong.");
    ui.blank();
    ui.say(
        "You are going to make one page, look at it, and then publish it.\n\
         This first part happens entirely on your own computer.\n\
         No account. No sign-up. Nothing leaves this machine.",
    );
    ui.blank();

    let mut onboarding = context.onboarding()?;

    // Already have a readable page? Then this is a resume, not a first run.
    if let Ok(workspace) = context.workspace() {
        if workspace.has_document() {
            match workspace.read_document() {
                Ok(document) => {
                    let filename = workspace.document_path().file_name().unwrap_or("kosong.md");
                    ui.status(
                        Mark::Good,
                        "you already have a page",
                        document.title_or_filename(filename),
                    );
                    ui.field("file", workspace.document_path().as_str());
                    ui.blank();

                    onboarding.local_document_created = true;
                    onboarding.workspace_path = Some(workspace.root().to_owned());
                    context.save_onboarding(&onboarding);

                    let step = onboarding.next_step();
                    ui.lesson(step.lesson());
                    ui.next_command(step.command());
                    return Ok(());
                }
                Err(error) => {
                    ui.status(Mark::Bad, "your page needs fixing", error.to_string());
                    ui.blank();
                    ui.next_command("kosong doctor");
                    return Err(error.into());
                }
            }
        }
    }

    // No page yet. Make one.
    let workspace = context.workspace_here()?;

    let title = ui
        .ask("What should the page be called? (press Enter for \"My First Site\")")
        .unwrap_or_else(|| "My First Site".to_owned());

    let document = Document::new(&title)?;
    workspace.create_document(&document)?;

    ui.blank();
    ui.status(
        Mark::Good,
        "made your page",
        workspace.document_path().as_str(),
    );
    ui.blank();
    ui.lesson(
        "That is a normal Markdown file. Nothing about it is specific to kosong,\n\
         and you can open it with any editor you already have.",
    );

    onboarding.local_document_created = true;
    onboarding.workspace_path = Some(workspace.root().to_owned());
    context.save_onboarding(&onboarding);

    // §7.2's flow is start → edit → preview, so editing is named directly
    // here. It is not a rung of the onboarding ladder, because §5.4 fixes the
    // permitted state fields and none of them records "has edited" — and a
    // page the user chose not to change is not an incomplete page.
    ui.next_command("kosong edit");
    Ok(())
}
