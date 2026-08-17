// ---------------------------------------------------------------------------
// site rollback
// ---------------------------------------------------------------------------

use super::*;
use kosong_core::providers::cloudflare::{self, CloudflareOperation};
use kosong_core::site::SiteState;

/// `kosong site rollback` — show deployment history and how to go back.
///
/// Lesson: published systems can be changed safely with history and intent.
pub fn rollback(context: &Context, approval: Approval) -> CliResult<()> {
    let ui = context.ui;
    let workspace = context.workspace()?;
    let site_root = locate_site(&workspace)?;
    let state = SiteState::load(&site_root).map_err(map_site_error)?;

    ui.heading(format!("Past versions of `{}`.", state.site_name));
    ui.blank();

    // The same diagnosis `publish` gives, so a rollback also explains an install
    // that reported success and then could not be found. Only the reason for
    // needing the tool differs.
    if crate::tools::locate(cloudflare::PROGRAM, Some(&site_root)).is_none() {
        report_wrangler_missing(ui, &site_root)?;
    }

    let operation =
        CloudflareOperation::deployment_list(state.project()).map_err(map_provider_error)?;

    if let Some(result) = perform(ui, &operation, &site_root, approval)? {
        if result.success() {
            ui.always(result.combined_output());
        } else {
            ui.warn("could not list past versions");
            ui.say(result.combined_output());
        }
    }

    ui.blank();
    for line in cloudflare::rollback_guidance(state.project()).lines() {
        ui.say(line);
    }
    Ok(())
}

/// Reports that wrangler is missing, tailored for `rollback`.
fn report_wrangler_missing(_ui: Ui, site_root: &Utf8Path) -> CliResult<()> {
    let install = format!(
        "Install it into your site folder, the way Cloudflare recommends:\n  \
         cd {site_root}\n  npm i -D wrangler@latest\n  npx wrangler login"
    );

    let (problem, repair) = match crate::tools::unreachable_install(cloudflare::PROGRAM) {
        Some(path) => (
            "wrangler is installed, but your shell cannot find it",
            format!(
                "kosong found it here:\n  {path}\n\
                 \n\
                 But this folder is not in your PATH, so nothing can run it by name.\n\
                 \n\
                 {install}"
            ),
        ),
        None => (
            "wrangler is needed to look up past versions",
            format!(
                "kosong looked in:\n  {local}\n  every folder in your PATH\n\n{install}",
                local = site_root.join(crate::tools::LOCAL_BIN),
            ),
        ),
    };

    Err(CliError::provider("WRANGLER_MISSING", problem).with_repair(repair))
}
