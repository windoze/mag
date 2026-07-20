//! File I/O (`load` / `save_atomic`) and structural-validation tests.
//! All tests are offline and use temp directories.

use mag_config::{ConfigDto, ConfigError, SecretRef};

fn sample_dto() -> ConfigDto {
    ConfigDto::parse_str(
        r#"
[providers.anthropic]
wire = "anthropic"
api_key = { env = "ANTHROPIC_API_KEY" }

[agents.default]
provider = "anthropic"
model = "claude-sonnet-4-5"
"#,
    )
    .unwrap()
}

#[test]
fn save_atomic_then_load_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("config.toml"); // parent created on demand

    let dto = sample_dto();
    dto.save_atomic(&path).unwrap();
    assert!(path.exists());

    let loaded = ConfigDto::load(&path).unwrap();
    assert_eq!(dto, loaded);
}

#[test]
fn save_atomic_overwrites_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    sample_dto().save_atomic(&path).unwrap();

    let mut updated = sample_dto();
    updated.providers.get_mut("anthropic").unwrap().api_key =
        Some(SecretRef::keyring("mag/anthropic"));
    updated.save_atomic(&path).unwrap();

    let loaded = ConfigDto::load(&path).unwrap();
    assert_eq!(loaded, updated);
    // No temp file is left behind.
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.contains(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");
}

#[test]
fn load_missing_file_reports_io_error_with_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("does-not-exist.toml");
    let err = ConfigDto::load(&path).unwrap_err();
    match err {
        ConfigError::Io { path: p, .. } => assert_eq!(p, path),
        other => panic!("expected Io error, got: {other:?}"),
    }
}

#[test]
fn parse_error_carries_line_and_column() {
    let input = "[providers.anthropic]\nwire = \"anthropic\"\nthis is not toml\n";
    let err = ConfigDto::parse_str(input).unwrap_err();
    match err {
        ConfigError::Parse {
            path,
            line,
            col,
            message,
        } => {
            assert!(path.is_none());
            assert_eq!(line, 3, "message: {message}");
            assert!(col >= 1);
        }
        other => panic!("expected Parse error, got: {other:?}"),
    }
}

#[test]
fn load_error_carries_file_path_and_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.toml");
    std::fs::write(&path, "[session]\nrouting = \n").unwrap();
    let err = ConfigDto::load(&path).unwrap_err();
    match err {
        ConfigError::Parse { path: p, line, .. } => {
            assert_eq!(p.as_deref(), Some(path.as_path()));
            assert_eq!(line, 2);
        }
        other => panic!("expected Parse error, got: {other:?}"),
    }
}

#[test]
fn invalid_secret_ref_reports_parse_error_with_line() {
    let input = "[providers.p]\napi_key = \"sk-inline-secret\"\n";
    let err = ConfigDto::parse_str(input).unwrap_err();
    match err {
        ConfigError::Parse { line, message, .. } => {
            assert_eq!(line, 2);
            assert!(
                message.contains("invalid secret reference"),
                "got: {message}"
            );
        }
        other => panic!("expected Parse error, got: {other:?}"),
    }
}

#[test]
fn validation_rejects_empty_section_name() {
    let mut dto = sample_dto();
    dto.providers.insert(String::new(), Default::default());
    let err = dto.validate().unwrap_err();
    match err {
        ConfigError::Validation { path, .. } => assert_eq!(path, "providers"),
        other => panic!("expected Validation error, got: {other:?}"),
    }
}

#[test]
fn validation_rejects_empty_string_values() {
    let mut dto = sample_dto();
    dto.providers.get_mut("anthropic").unwrap().wire = Some("  ".into());
    let err = dto.validate().unwrap_err();
    match err {
        ConfigError::Validation { path, .. } => {
            assert_eq!(path, "providers.anthropic.wire")
        }
        other => panic!("expected Validation error, got: {other:?}"),
    }
}

#[test]
fn validation_rejects_duplicate_agent_tools_with_indexed_path() {
    let mut dto = sample_dto();
    dto.agents.get_mut("default").unwrap().tools = Some(vec![
        "read_file".to_string(),
        "grep".to_string(),
        "read_file".to_string(),
    ]);
    let err = dto.validate().unwrap_err();
    match err {
        ConfigError::Validation { path, message } => {
            assert_eq!(path, "agents.default.tools[2]");
            assert!(message.contains("duplicate"), "got: {message}");
        }
        other => panic!("expected Validation error, got: {other:?}"),
    }
}

#[test]
fn validation_rejects_empty_external_command() {
    let mut dto = sample_dto();
    dto.external_agents.insert(
        "peer".to_string(),
        mag_config::ExternalAgentDto {
            kind: Some("acp".into()),
            command: Some(vec![]),
            ..Default::default()
        },
    );
    let err = dto.validate().unwrap_err();
    match err {
        ConfigError::Validation { path, .. } => {
            assert_eq!(path, "external_agents.peer.command")
        }
        other => panic!("expected Validation error, got: {other:?}"),
    }
}

#[test]
fn validation_rejects_zero_budget_and_zero_timeout() {
    let mut dto = sample_dto();
    dto.session = Some(mag_config::SessionDefaultsDto {
        budget: Some(mag_config::BudgetDto {
            max_tokens: Some(0),
            ..Default::default()
        }),
        ..Default::default()
    });
    let err = dto.validate().unwrap_err();
    match err {
        ConfigError::Validation { path, .. } => {
            assert_eq!(path, "session.budget.max_tokens")
        }
        other => panic!("expected Validation error, got: {other:?}"),
    }

    let mut dto = sample_dto();
    dto.approval = Some(mag_config::ApprovalSectionDto {
        default_policy: Some("ask".into()),
        timeout_secs: Some(0),
    });
    let err = dto.validate().unwrap_err();
    match err {
        ConfigError::Validation { path, .. } => assert_eq!(path, "approval.timeout_secs"),
        other => panic!("expected Validation error, got: {other:?}"),
    }
}

#[test]
fn load_runs_validation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[agents.a]\ntools = [\"x\", \"x\"]\n").unwrap();
    let err = ConfigDto::load(&path).unwrap_err();
    match err {
        ConfigError::Validation { path, .. } => assert_eq!(path, "agents.a.tools[1]"),
        other => panic!("expected Validation error, got: {other:?}"),
    }
}
