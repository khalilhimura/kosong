//! `kosong new` — create a page.
//!
//! Lesson: a file is a durable object you can inspect and move.

use super::Context;
use crate::exit::{CliError, CliResult};
use camino::Utf8PathBuf;
use kosong_core::Workspace;
use kosong_core::okf::{self, Document};

pub fn run(context: &Context, path: Option<Utf8PathBuf>, title: Option<String>) -> CliResult<()> {
    let ui = context.ui;

    let (workspace, filename) = resolve_target(context, path)?;
    let title = title.unwrap_or_else(|| "My First Site".to_owned());

    let document = Document::new(&title)?;
    let target = workspace.root().join(&filename);

    // A workspace opened for a non-default filename still writes through
    // `Workspace`, so symlink and containment checks apply either way.
    let workspace = Workspace::open_with_document(workspace.root(), &filename)?;
    workspace.create_document(&document)?;

    let meta = document
        .kosong()?
        .expect("a document kosong just created is managed");

    ui.heading("Created your page.");
    ui.blank();
    ui.field("file", target.as_str());
    ui.field("title", &title);
    ui.field("id", &meta.id);
    ui.blank();
    ui.lesson(
        "This is an ordinary Markdown file. You can open it in any editor, move it, \n\
         copy it, or put it in a folder you sync. kosong does not own it.",
    );

    let mut onboarding = context.onboarding()?;
    onboarding.local_document_created = true;
    onboarding.workspace_path = Some(workspace.root().to_owned());
    context.save_onboarding(&onboarding);

    ui.next_command("kosong edit");
    Ok(())
}

/// Works out which folder and filename to write.
///
/// `--path` may name a file or a folder. A trailing component ending in `.md`
/// is treated as the filename; anything else is treated as a folder.
fn resolve_target(context: &Context, path: Option<Utf8PathBuf>) -> CliResult<(Workspace, String)> {
    let Some(path) = path else {
        return Ok((context.workspace_here()?, okf::DEFAULT_FILENAME.to_owned()));
    };

    let absolute = if path.is_absolute() {
        path
    } else {
        context.here.join(path)
    };

    if absolute.as_str().ends_with(".md") {
        let filename = absolute
            .file_name()
            .ok_or_else(|| {
                CliError::usage("BAD_PATH", format!("`{absolute}` has no file name"))
                    .with_repair("Give a path ending in a file name, such as ./kosong.md")
            })?
            .to_owned();

        let parent = absolute.parent().unwrap_or(&context.here).to_owned();
        std::fs::create_dir_all(&parent).map_err(|e| {
            CliError::usage("BAD_PATH", format!("could not create `{parent}`: {e}"))
        })?;

        Ok((Workspace::open(&parent)?, filename))
    } else {
        std::fs::create_dir_all(&absolute).map_err(|e| {
            CliError::usage("BAD_PATH", format!("could not create `{absolute}`: {e}"))
        })?;
        Ok((
            Workspace::open(&absolute)?,
            okf::DEFAULT_FILENAME.to_owned(),
        ))
    }
}
