//! `kosong sync` — keep a private remote copy of the page.
//!
//! FR-4: a bare `sync` explains its detected action or the conflict;
//! `--push` and `--pull` are explicit overrides. Conflicts are visible and
//! non-destructive, and v1 never silently merges.
//!
//! The decision itself lives in [`kosong_core::sync::plan`], which is pure.
//! This module is the part that does I/O and talks to the user.

use super::Context;
use crate::exit::{CliError, CliResult};
use crate::remote::Remote;
use crate::ui::Mark;
use kosong_core::Workspace;
use kosong_core::okf::{self, Document};
use kosong_core::sync::{self, LocalSide, RemoteSide, SyncAction, SyncState};

/// Which way the user asked to sync, if they said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Work it out and explain.
    Detect,
    Push,
    Pull,
}

pub fn run(context: &Context, direction: Direction) -> CliResult<()> {
    let ui = context.ui;
    // Not `workspace()`: a pull onto a second machine begins with an empty
    // folder, and requiring a document first would break the one case sync
    // exists for.
    let workspace = context.workspace_or_here()?;
    let remote = Remote::open(&context.api_base_url()?)?;

    if !remote.is_signed_in() {
        return Err(crate::remote::not_signed_in());
    }

    // Read local first. A document that will not parse must not be pushed, and
    // finding that out before any network call keeps the failure cheap.
    let local_bytes = if workspace.has_document() {
        let document = workspace.read_document()?;
        Some(document.to_okf_string()?.into_bytes())
    } else {
        None
    };

    let remote_document = remote.with_access_token(|token| {
        let client = remote.client().clone();
        async move { client.get_document(&token).await }
    })?;

    let local_side = local_bytes.as_ref().map(|bytes| LocalSide {
        hash: sync::content_hash(bytes),
    });
    let remote_side = remote_document.as_ref().map(|document| RemoteSide {
        etag: document.etag.clone(),
        hash: Some(sync::content_hash(&document.bytes)),
    });

    let state = SyncState::load(&workspace);
    let action = match direction {
        Direction::Detect => sync::plan(local_side.as_ref(), remote_side.as_ref(), &state),
        Direction::Push => {
            if local_side.is_none() {
                return Err(
                    CliError::usage("NOTHING_TO_PUSH", "there is no page here to send")
                        .with_repair("Run: kosong new"),
                );
            }
            // An explicit --push still respects the remote version it knows
            // about, so a stale client is told rather than allowed to clobber.
            SyncAction::Push {
                if_match: remote_side
                    .as_ref()
                    .map(|r| r.etag.clone())
                    .unwrap_or_else(|| "*".to_owned()),
            }
        }
        Direction::Pull => {
            if remote_side.is_none() {
                return Err(CliError::usage(
                    "NOTHING_TO_PULL",
                    "there is no page on the server yet",
                )
                .with_repair("Run: kosong sync --push"));
            }
            SyncAction::Pull
        }
    };

    if direction == Direction::Detect {
        ui.say(action.describe());
        ui.blank();
    }

    match action {
        SyncAction::NothingToSync => {
            ui.status(Mark::Info, "nothing to sync", "");
            ui.next_command("kosong new");
            Ok(())
        }

        SyncAction::UpToDate => {
            // Record the agreement so a later sync has a baseline and does not
            // read as a conflict.
            if let (Some(local), Some(remote_side)) = (&local_side, &remote_side) {
                save_state(&workspace, &remote_side.etag, &local.hash, ui);
            }
            ui.status(Mark::Good, "already in sync", "");
            Ok(())
        }

        SyncAction::Push { if_match } => {
            let bytes = local_bytes.expect("a push requires a local document");
            push(
                context,
                &remote,
                &workspace,
                &bytes,
                &if_match,
                remote_document.as_ref(),
            )
        }

        SyncAction::Pull => {
            let document = remote_document.expect("a pull requires a remote document");
            pull(context, &workspace, &document, local_bytes.as_deref())
        }

        SyncAction::Conflict => {
            let local = local_bytes.expect("a conflict requires a local document");
            let document = remote_document.expect("a conflict requires a remote document");
            report_conflict(context, &workspace, &local, &document.bytes)
        }
    }
}

fn push(
    context: &Context,
    remote: &Remote,
    workspace: &Workspace,
    bytes: &[u8],
    if_match: &str,
    remote_document: Option<&kosong_core::api::RemoteDocument>,
) -> CliResult<()> {
    let ui = context.ui;

    let result = remote.with_access_token(|token| {
        let client = remote.client().clone();
        let payload = bytes.to_vec();
        let if_match = if_match.to_owned();
        async move { client.put_document(&token, &payload, &if_match).await }
    });

    match result {
        Ok(written) => {
            save_state(workspace, &written.etag, &sync::content_hash(bytes), ui);
            ui.status(Mark::Good, "sent to the server", &written.updated_at);
            ui.lesson(
                "Your page is stored privately. Only you can read it, and it is not published \
                 by syncing.",
            );
            Ok(())
        }
        Err(error) if error.code == "DOCUMENT_CONFLICT" => {
            // Someone wrote between the read and the write. Both versions are
            // preserved rather than one being lost to a race.
            let remote_bytes = remote_document.map(|d| d.bytes.clone()).unwrap_or_default();
            report_conflict(context, workspace, bytes, &remote_bytes)
        }
        Err(error) => Err(error),
    }
}

fn pull(
    context: &Context,
    workspace: &Workspace,
    document: &kosong_core::api::RemoteDocument,
    local_bytes: Option<&[u8]>,
) -> CliResult<()> {
    let ui = context.ui;

    let text = String::from_utf8(document.bytes.clone()).map_err(|_| {
        CliError::usage(
            "REMOTE_NOT_TEXT",
            "the page on the server is not readable text",
        )
        .with_repair("This should not happen. Please report it.")
    })?;

    // Validated before it lands on disk: a bad remote document must not
    // replace a good local one.
    let parsed = Document::parse(&text)?;

    // If there is local content that was never synced, keep a copy. Nothing
    // the user typed should vanish because they ran a pull.
    if let Some(local) = local_bytes {
        if local != document.bytes.as_slice() {
            let snapshot =
                sync::write_conflict(workspace, local, &document.bytes, &okf::now_rfc3339())?;
            ui.warn(format!(
                "Your local page differed, so a copy was kept at {}",
                snapshot.local_path
            ));
        }
    }

    workspace.write_document(&parsed)?;
    save_state(
        workspace,
        &document.etag,
        &sync::content_hash(&document.bytes),
        ui,
    );

    ui.status(
        Mark::Good,
        "copied from the server",
        workspace.document_path().as_str(),
    );
    ui.next_command("kosong show");
    Ok(())
}

/// Preserves both versions and hands the decision to the user.
///
/// Returns an error so the exit code reflects that the sync did not complete,
/// while the output makes clear that nothing was lost.
fn report_conflict(
    context: &Context,
    workspace: &Workspace,
    local_bytes: &[u8],
    remote_bytes: &[u8],
) -> CliResult<()> {
    let ui = context.ui;
    let snapshot = sync::write_conflict(workspace, local_bytes, remote_bytes, &okf::now_rfc3339())?;

    ui.blank();
    ui.status(
        Mark::Warn,
        "both versions changed",
        "nothing was overwritten",
    );
    ui.blank();
    ui.field("yours", snapshot.local_path.as_str());
    ui.field("server's", snapshot.remote_path.as_str());
    ui.blank();
    ui.lesson(
        "kosong will not merge two versions of your writing, because it cannot know which \n\
         parts you meant to keep. Both are saved above, unchanged.",
    );

    Err(CliError::usage(
        "SYNC_CONFLICT",
        "your page and the server's have both changed",
    )
    .with_repair(format!(
        "Open both files, decide what the page should say, then save it as:\n  {}\n\
         Once it is right, run:\n  kosong sync --push",
        workspace.document_path()
    )))
}

/// Records a successful sync.
///
/// A failure here is a warning, not an error: the sync itself succeeded, and
/// the only cost of a missing record is a conflict prompt next time.
fn save_state(workspace: &Workspace, etag: &str, hash: &str, ui: crate::ui::Ui) {
    let state = SyncState {
        etag: Some(etag.to_owned()),
        content_hash: Some(hash.to_owned()),
        synced_at: Some(okf::now_rfc3339()),
    };
    if let Err(error) = state.save(workspace) {
        ui.warn(format!("could not remember this sync: {error}"));
    }
}
