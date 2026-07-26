//! `kosong delete-account` — remove the account and everything stored for it.
//!
//! FR-3 requires that a user be able to delete their account and their remote
//! document. The deletion itself is the server's work; what this command owes
//! the user is an exact account of its scope *before* it becomes irreversible.
//!
//! The scope is the part people get wrong. Deleting the account removes what
//! is on the server and nothing else: the page on this computer, its git
//! history, and any site already published to Cloudflare all survive. Someone
//! who deletes their account believing it takes their public page down would
//! be worse off for having used kosong.

use super::Context;
use crate::exit::{CliError, CliResult};
use crate::remote::Remote;
use crate::ui::Mark;

pub fn delete(context: &Context, dry_run: bool, assume_yes: bool) -> CliResult<()> {
    let ui = context.ui;
    let remote = Remote::open(&context.api_base_url()?)?;

    // Checked locally so the common mistake — running this while signed out —
    // costs nothing and says the same thing every other account command says.
    if !remote.is_signed_in() {
        return Err(crate::remote::not_signed_in());
    }

    disclose(ui);

    if dry_run {
        ui.status(Mark::Info, "dry run", "nothing was deleted");
        return Ok(());
    }

    // Default no. This cannot be undone, and a non-interactive stdin takes the
    // default — so a script that never meant to delete anything cannot, and
    // one that does must say `--yes` in as many words.
    if !assume_yes && !ui.confirm("Delete the account? This cannot be undone.", false) {
        return Err(
            CliError::usage("CANCELLED", "stopped before deleting anything").with_repair(
                "Nothing was deleted. Your account and the page on the server are unchanged.",
            ),
        );
    }

    remote.with_access_token(|token| {
        let client = remote.client().clone();
        async move { client.delete_account(&token).await }
    })?;

    // The server revoked every session as part of the deletion, so what is
    // stored here is now a dead credential. Keeping it would make the next
    // command fail as though something had gone wrong.
    //
    // This runs only after the deletion succeeded: a network failure must
    // leave the sign-in intact, because the account is still there.
    remote.forget()?;

    let mut onboarding = context.onboarding()?;
    onboarding.login_completed = false;
    context.save_onboarding(&onboarding);

    ui.blank();
    ui.status(
        Mark::Good,
        "deleted",
        "the account and its stored page are gone",
    );
    ui.lesson(
        "Your page is still on this computer, and so is anything you published.\n\
         Deleting an account deletes an account; it does not reach into your files.",
    );
    ui.next_command("kosong status");
    Ok(())
}

/// States the full effect before anything happens, per §7 of the CLI contract.
///
/// Both halves are said out loud. A list of what is destroyed, with no list of
/// what survives, leaves the user to guess — and the thing they most need to
/// know is that a published site is not covered.
fn disclose(ui: crate::ui::Ui) {
    ui.heading("Deleting your kosong account.");
    ui.blank();

    ui.say("On the server, this removes:");
    ui.field(
        "account",
        "your sign-in, and every session on every computer",
    );
    ui.field("page", "the private copy kept by `kosong sync`");
    ui.blank();

    ui.say("On this computer, this removes nothing:");
    ui.field("your page", "kosong.md stays exactly where it is");
    ui.field("your site", "the site folder and its git history stay");
    ui.blank();

    ui.say("Already published? That stays online too.");
    ui.lesson(
        "A published site lives in your GitHub repository and your Cloudflare account,\n\
         neither of which kosong can reach. Take it down there if you want it gone.",
    );
    ui.blank();
}
