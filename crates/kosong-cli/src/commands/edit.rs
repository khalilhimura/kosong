//! `kosong edit` — open the page in the user's editor.
//!
//! Lesson: the terminal can open normal editors; it is not an editor itself.

use super::Context;
use crate::editor::{self, EditorCommand};
use crate::exit::CliResult;

pub fn run(context: &Context, editor_override: Option<String>, dry_run: bool) -> CliResult<()> {
    let ui = context.ui;
    let workspace = context.workspace()?;

    // Validate before opening. If the file is already broken, saying so now is
    // more useful than letting the user edit and discover it afterwards.
    let before = workspace.read_document()?;

    let setting = match editor_override {
        Some(value) => Some(value),
        None => context.config()?.resolve_editor(),
    };
    let Some(setting) = setting else {
        return Err(editor::no_editor_error());
    };

    let Some(command) = EditorCommand::parse(&setting)? else {
        return Err(editor::no_editor_error());
    };

    let path = workspace.document_path().to_owned();

    if dry_run {
        ui.say("Would run:");
        ui.say(format!("  {}", command.display(&path)));
        ui.blank();
        ui.lesson("kosong starts your editor directly. Nothing is passed through a shell.");
        return Ok(());
    }

    ui.say(format!("Opening {} in {}…", path, command.program));
    command.launch(&path)?;

    // Re-read so a broken save is reported now rather than at publish time.
    match workspace.read_document() {
        Ok(after) => {
            if after.body() == before.body() {
                ui.say("No changes saved.");
            } else {
                ui.say("Saved.");
            }
            ui.next_command("kosong preview");
            Ok(())
        }
        Err(error) => {
            ui.warn("the file was saved, but kosong can no longer read it");
            Err(error.into())
        }
    }
}
