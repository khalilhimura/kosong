//! `kosong login` — sign in with a six-digit email code.
//!
//! Lesson: signing in keeps a private copy you can pull back later.
//!
//! No password is ever asked for or stored. The code arrives by email, works
//! once, and expires in ten minutes.

use super::Context;
use crate::exit::{CliError, CliResult, Exit};
use crate::remote::{Remote, device_name, map_api_error};
use crate::ui::Mark;

pub fn run(context: &Context, email: Option<String>, code: Option<String>) -> CliResult<()> {
    let ui = context.ui;
    let remote = Remote::open(&context.api_base_url()?)?;

    if remote.is_signed_in() {
        // A supplied --email is a request to sign in *as that address*.
        // Reporting success while quietly staying signed in as somebody else
        // would leave the user operating one account believing they are on
        // another — so this fails rather than no-ops.
        if email.is_some() {
            return Err(CliError::usage(
                "ALREADY_SIGNED_IN",
                "you are already signed in as somebody else",
            )
            .with_repair("Sign out first, then sign in again:\n  kosong logout\n  kosong login"));
        }

        ui.status(Mark::Good, "you are already signed in", "");
        ui.lesson("To sign in as someone else, run `kosong logout` first.");
        return Ok(());
    }

    let email = match email {
        Some(value) => value,
        None => ui
            .ask("What email address should kosong use?")
            .ok_or_else(|| {
                CliError::usage("NO_EMAIL", "kosong needs an email address to sign you in")
                    .with_repair("Run: kosong login --email you@example.com")
            })?,
    };

    // Validated locally first, so an obvious typo costs nothing and does not
    // consume one of the address's rate-limited code requests.
    if !looks_like_an_email(&email) {
        return Err(CliError::usage(
            "BAD_EMAIL",
            format!("`{email}` does not look like an email address"),
        )
        .with_repair("Check the spelling and try again."));
    }

    // A supplied --code means the caller already has one from an earlier
    // request, so asking the server for another would burn a rate-limit slot
    // and invalidate the code they are holding.
    let code = match code {
        Some(supplied) => supplied,
        None => {
            ui.say("Sending a code…");
            remote
                .block_on(remote.client().request_code(&email))
                .map_err(map_api_error)?;

            ui.blank();
            ui.status(Mark::Good, "sent", "check your email for a six-digit code");
            ui.lesson("The code works once and expires in 10 minutes.");
            ui.blank();

            ui.ask("Code:").ok_or_else(|| {
                CliError::usage("NO_CODE", "no code was entered")
                    .with_repair("Run `kosong login` again when you have the code from your email.")
            })?
        }
    };

    // People paste codes with spaces or hyphens from an email client.
    let code = code.trim().replace([' ', '-'], "");

    let device = device_name();
    let result = remote
        .block_on(
            remote
                .client()
                .verify_code(&email, &code, Some(device.as_str())),
        )
        .map_err(map_api_error)?;

    remote.remember(&result.tokens.refresh_token)?;

    ui.blank();
    ui.status(Mark::Good, "signed in", &result.user.email_hint);

    // §6.4 requires saying so prominently when the keychain is unavailable.
    // Quietly degrading would leave the user believing their credential is
    // better protected than it is.
    if remote.backend().warrants_warning() {
        ui.blank();
        ui.warn(format!(
            "Your system keychain was not available, so kosong kept your sign-in in {} \
             that only you can read.",
            remote.backend().describe()
        ));
    } else {
        ui.field("kept in", remote.backend().describe());
    }

    let mut onboarding = context.onboarding()?;
    onboarding.login_completed = true;
    context.save_onboarding(&onboarding);

    ui.next_command("kosong sync");
    Ok(())
}

/// A deliberately loose check: one `@`, something either side, a dot after.
///
/// The server validates properly. This exists only to catch a typo before it
/// costs the user one of their rate-limited requests, so it must not reject
/// anything the server would accept.
fn looks_like_an_email(candidate: &str) -> bool {
    let candidate = candidate.trim();
    let Some((local, domain)) = candidate.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !candidate.contains(char::is_whitespace)
}

/// `kosong logout` — end the session.
pub fn logout(context: &Context) -> CliResult<()> {
    let ui = context.ui;
    let remote = Remote::open(&context.api_base_url()?)?;

    let Some(refresh_token) = remote.store().load().map_err(|e| {
        let repair = e.repair();
        CliError::new(Exit::Auth, "SESSION_READ_FAILED", e.to_string()).with_repair(repair)
    })?
    else {
        ui.status(Mark::Info, "you were not signed in", "");
        return Ok(());
    };

    // The local credential is cleared even if the server cannot be reached.
    // Leaving a token on disk because the network was down would be the wrong
    // way to fail: the user asked to be signed out.
    let server_result = remote.block_on(remote.client().logout(&refresh_token));
    remote.forget()?;

    match server_result {
        Ok(()) => {
            ui.status(Mark::Good, "signed out", "");
        }
        Err(error) => {
            ui.status(Mark::Good, "signed out on this computer", "");
            ui.warn(format!(
                "The server could not be reached, so the session may still be open elsewhere ({error}). \
                 It will expire on its own."
            ));
        }
    }

    let mut onboarding = context.onboarding()?;
    onboarding.login_completed = false;
    context.save_onboarding(&onboarding);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::looks_like_an_email;

    #[test]
    fn ordinary_addresses_pass() {
        for address in [
            "person@example.com",
            "first.last@example.co.uk",
            "user+tag@example.com",
            "a@b.co",
        ] {
            assert!(looks_like_an_email(address), "`{address}` should pass");
        }
    }

    #[test]
    fn obvious_typos_are_caught() {
        for address in [
            "",
            "person",
            "person@",
            "@example.com",
            "person@example",
            "a b@c.com",
        ] {
            assert!(
                !looks_like_an_email(address),
                "`{address}` should be caught"
            );
        }
    }
}
