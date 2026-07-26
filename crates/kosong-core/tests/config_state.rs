//! Configuration and onboarding state.
//!
//! The rule these tests defend: technical specification §5.4 forbids storing
//! verification codes, access tokens, refresh tokens, document content, or
//! provider credentials in onboarding state.

use camino::Utf8PathBuf;
use kosong_core::config::{Config, ConfigError, NextStep, Onboarding, SCHEMA_VERSION};
use tempfile::TempDir;

fn temp() -> (TempDir, Utf8PathBuf) {
    let dir = TempDir::new().expect("temp dir");
    let path = std::fs::canonicalize(dir.path()).expect("canonicalize");
    (dir, Utf8PathBuf::from_path_buf(path).expect("utf-8 path"))
}

// ---------------------------------------------------------------------------
// config.toml
// ---------------------------------------------------------------------------

#[test]
fn a_missing_config_file_yields_defaults_rather_than_an_error() {
    let (_guard, root) = temp();

    let config = Config::load_from(&root.join("config.toml")).unwrap();

    assert_eq!(config, Config::default());
    assert_eq!(config.version, SCHEMA_VERSION);
}

#[test]
fn config_survives_a_save_and_load_cycle() {
    let (_guard, root) = temp();
    let path = root.join("config.toml");

    let config = Config {
        version: SCHEMA_VERSION,
        editor: Some("nano".into()),
        api_base_url: Some("https://api.example.test".into()),
        preview_port: Some(5000),
    };
    config.save_to(&path).unwrap();

    assert_eq!(Config::load_from(&path).unwrap(), config);
}

#[test]
fn unset_options_are_omitted_from_the_file() {
    let (_guard, root) = temp();
    let path = root.join("config.toml");
    Config::default().save_to(&path).unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("editor"));
    assert!(!text.contains("api_base_url"));
    assert!(text.contains("version"));
}

#[test]
fn a_malformed_config_reports_where_and_how_to_fix_it() {
    let (_guard, root) = temp();
    let path = root.join("config.toml");
    std::fs::write(&path, "this is not = valid = toml").unwrap();

    let err = Config::load_from(&path).unwrap_err();

    assert!(matches!(err, ConfigError::Malformed { .. }));
    assert!(err.repair().contains("config.toml"));
    assert!(err.repair().contains("kosong doctor"));
}

#[test]
fn the_preview_port_defaults_when_unset() {
    assert_eq!(Config::default().preview_port(), 4173);

    let configured = Config {
        preview_port: Some(9999),
        ..Config::default()
    };
    assert_eq!(configured.preview_port(), 9999);
}

#[test]
fn a_configured_editor_wins_over_the_environment() {
    let config = Config {
        editor: Some("my-editor".into()),
        ..Config::default()
    };
    assert_eq!(config.resolve_editor().as_deref(), Some("my-editor"));
}

#[test]
fn a_blank_editor_setting_is_ignored() {
    let config = Config {
        editor: Some("   ".into()),
        ..Config::default()
    };
    assert_eq!(config.resolve_editor(), None);
}

// ---------------------------------------------------------------------------
// onboarding.toml
// ---------------------------------------------------------------------------

#[test]
fn onboarding_starts_empty() {
    let (_guard, root) = temp();

    let state = Onboarding::load_from(&root.join("onboarding.toml")).unwrap();

    assert!(!state.local_document_created);
    assert!(!state.preview_completed);
    assert!(!state.login_completed);
    assert!(!state.site_initialized);
    assert!(!state.site_published);
}

#[test]
fn onboarding_survives_a_save_and_load_cycle() {
    let (_guard, root) = temp();
    let path = root.join("onboarding.toml");

    let state = Onboarding {
        version: SCHEMA_VERSION,
        workspace_path: Some(root.clone()),
        local_document_created: true,
        preview_completed: true,
        login_completed: false,
        site_initialized: false,
        site_published: false,
    };
    state.save_to(&path).unwrap();

    assert_eq!(Onboarding::load_from(&path).unwrap(), state);
}

#[test]
fn onboarding_state_contains_only_the_permitted_fields() {
    // Serializing a fully populated state must not produce any field capable
    // of holding a secret. If someone adds one, this test fails.
    let (_guard, root) = temp();
    let path = root.join("onboarding.toml");

    Onboarding {
        version: SCHEMA_VERSION,
        workspace_path: Some(root.clone()),
        local_document_created: true,
        preview_completed: true,
        login_completed: true,
        site_initialized: true,
        site_published: true,
    }
    .save_to(&path)
    .unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    let keys: Vec<&str> = text
        .lines()
        .filter_map(|line| line.split('=').next())
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .collect();

    let permitted = [
        "version",
        "workspace_path",
        "local_document_created",
        "preview_completed",
        "login_completed",
        "site_initialized",
        "site_published",
    ];
    for key in &keys {
        assert!(
            permitted.contains(key),
            "`{key}` is not a permitted onboarding field"
        );
    }

    for forbidden in [
        "token", "code", "secret", "password", "refresh", "access", "email",
    ] {
        assert!(
            !text.to_lowercase().contains(forbidden),
            "onboarding state must never contain `{forbidden}`"
        );
    }
}

#[test]
fn a_secret_written_by_hand_is_dropped_rather_than_preserved() {
    // Unlike the OKF document, these are kosong's own files, and forgetting an
    // unexpected value is the safer behaviour here.
    let (_guard, root) = temp();
    let path = root.join("onboarding.toml");
    std::fs::write(
        &path,
        "version = 1\nlocal_document_created = true\npreview_completed = false\n\
         login_completed = false\nsite_initialized = false\nsite_published = false\n\
         access_token = \"leaked-secret-value\"\n",
    )
    .unwrap();

    let state = Onboarding::load_from(&path).unwrap();
    state.save_to(&path).unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains("leaked-secret-value"),
        "a secret must not survive a rewrite:\n{text}"
    );
}

// ---------------------------------------------------------------------------
// The onboarding ladder
// ---------------------------------------------------------------------------

#[test]
fn the_ladder_starts_at_creating_a_document() {
    assert_eq!(Onboarding::default().next_step(), NextStep::CreateDocument);
}

#[test]
fn local_work_comes_before_any_account() {
    // The product's core promise: the first win requires no account.
    let state = Onboarding {
        local_document_created: true,
        ..Onboarding::default()
    };
    assert_eq!(state.next_step(), NextStep::Preview);

    let previewed = Onboarding {
        preview_completed: true,
        ..state
    };
    assert_eq!(previewed.next_step(), NextStep::Login);
}

#[test]
fn the_ladder_runs_to_completion_in_order() {
    let mut state = Onboarding::default();
    let mut seen = Vec::new();

    for _ in 0..6 {
        let step = state.next_step();
        seen.push(step);
        match step {
            NextStep::CreateDocument => state.local_document_created = true,
            NextStep::Preview => state.preview_completed = true,
            NextStep::Login => state.login_completed = true,
            NextStep::InitSite => state.site_initialized = true,
            NextStep::Publish => state.site_published = true,
            NextStep::Done => {}
        }
    }

    assert_eq!(
        seen,
        vec![
            NextStep::CreateDocument,
            NextStep::Preview,
            NextStep::Login,
            NextStep::InitSite,
            NextStep::Publish,
            NextStep::Done,
        ]
    );
}

#[test]
fn every_step_offers_a_command_and_a_lesson() {
    for step in [
        NextStep::CreateDocument,
        NextStep::Preview,
        NextStep::Login,
        NextStep::InitSite,
        NextStep::Publish,
        NextStep::Done,
    ] {
        assert!(step.command().starts_with("kosong "));
        let lesson = step.lesson();
        assert!(!lesson.is_empty());
        assert!(lesson.ends_with('.'), "lessons are sentences: {lesson:?}");
    }
}
