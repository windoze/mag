//! `SecretRef` acceptance/rejection tests at the TOML boundary.

use mag_config::{ConfigDto, SecretRef};

fn parse_api_key(toml_fragment: &str) -> Result<SecretRef, String> {
    let input = format!("[providers.p]\napi_key = {toml_fragment}\n");
    ConfigDto::parse_str(&input)
        .map(|dto| dto.provider("p").unwrap().api_key.clone().unwrap())
        .map_err(|e| e.to_string())
}

#[test]
fn canonical_table_forms_parse() {
    assert_eq!(
        parse_api_key(r#"{ env = "ANTHROPIC_API_KEY" }"#).unwrap(),
        SecretRef::env("ANTHROPIC_API_KEY")
    );
    assert_eq!(
        parse_api_key(r#"{ keyring = "mag/local_proxy" }"#).unwrap(),
        SecretRef::keyring("mag/local_proxy")
    );
}

#[test]
fn string_dsl_forms_parse() {
    assert_eq!(
        parse_api_key(r#""env:ANTHROPIC_API_KEY""#).unwrap(),
        SecretRef::env("ANTHROPIC_API_KEY")
    );
    assert_eq!(
        parse_api_key(r#""keyring:mag/local_proxy""#).unwrap(),
        SecretRef::keyring("mag/local_proxy")
    );
}

#[test]
fn plain_secret_values_are_rejected() {
    // A bare string without the env:/keyring: prefix must not parse as a
    // reference — secrets can never be inlined by accident.
    let err = parse_api_key(r#""sk-ant-real-secret-value""#).unwrap_err();
    assert!(err.contains("invalid secret reference"), "got: {err}");
}

#[test]
fn malformed_reference_tables_are_rejected() {
    // Both kinds at once.
    assert!(parse_api_key(r#"{ env = "A", keyring = "B" }"#).is_err());
    // Unknown key.
    assert!(parse_api_key(r#"{ vault = "A" }"#).is_err());
    // Empty table.
    assert!(parse_api_key(r#"{}"#).is_err());
    // Empty name.
    assert!(parse_api_key(r#"{ env = "" }"#).is_err());
    // Unknown string-DSL kind.
    assert!(parse_api_key(r#""vault:secret""#).is_err());
}

#[test]
fn serialization_always_emits_canonical_table_form() {
    // Even when the input used the string DSL, the persisted form is the
    // §4.2 inline table.
    let dto = ConfigDto::parse_str("[providers.p]\napi_key = \"env:MY_VAR\"\n").unwrap();
    let out = dto.to_string_pretty().unwrap();
    assert!(out.contains("env = \"MY_VAR\""), "got:\n{out}");
    let reparsed = ConfigDto::parse_str(&out).unwrap();
    assert_eq!(reparsed, dto);
}
