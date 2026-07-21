//! Subagent definition model and markdown definition-file parsing.
//!
//! A subagent *definition* (`docs/dyn-agents.md` §3) is a declarative,
//! source-agnostic description of an agent type that the runtime `agent`
//! tool can instantiate. All provenances ([`DefinitionSource`]: builtin
//! constants, user markdown files under `~/.config/mag/agents/`, project
//! markdown files under `.mag/agents/`, TOML configuration entries) share the
//! one [`AgentDefinition`] model.
//!
//! This module implements the model and the markdown definition-file format
//! (§3.1: YAML frontmatter + markdown body) via [`parse_agent_md`]. Registry
//! assembly — multi-source discovery, merge priority, builtin texts, and the
//! TOML projection — is the M2-2 follow-up.

use std::collections::BTreeMap;

/// Fields recognized in the frontmatter mapping (`docs/dyn-agents.md` §3.1
/// field table). Anything else is an [`AgentDefError::UnknownField`].
const KNOWN_FIELDS: [&str; 8] = [
    "name",
    "description",
    "kind",
    "tools",
    "model",
    "max_steps",
    "command",
    "env",
];

/// Provenance of an [`AgentDefinition`].
///
/// Declaration order encodes the merge priority (`docs/dyn-agents.md` §3.2):
/// on a name clash a definition from a higher source replaces the lower one,
/// so `Builtin < User < Project < Toml` (the derived `Ord` matches).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DefinitionSource {
    /// Compiled into the binary (`general-purpose`, `explorer`).
    Builtin,
    /// Markdown file under the user config directory
    /// (`~/.config/mag/agents/`).
    User,
    /// Markdown file under the project directory (`<project>/.mag/agents/`).
    Project,
    /// Projected from TOML configuration (`[agents.<name>]` non-bound entries
    /// and `[external_agents.<name>]`).
    Toml,
}

/// Kind-specific payload of an [`AgentDefinition`] (`docs/dyn-agents.md`
/// §3.1 field table).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentKindDef {
    /// A local instance: driven in-process on the supervisor's LLM client.
    Local {
        /// Model override; `None` inherits the supervisor's model.
        model: Option<String>,
        /// Tool-name allowlist; `None` inherits the supervisor tool surface,
        /// `Some` is intersected with it (a definition can only narrow the
        /// surface, never widen it, §3.1).
        tools: Option<Vec<String>>,
        /// Step-budget override; `None` uses the runtime default.
        max_steps: Option<u32>,
    },
    /// An external ACP peer: spawned as a subprocess per instance.
    ExternalAcp {
        /// Spawn command line in argv form (never empty).
        command: Vec<String>,
        /// Extra environment for the spawned process.
        env: BTreeMap<String, String>,
    },
}

/// A source-agnostic subagent definition (`docs/dyn-agents.md` §3).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentDefinition {
    /// Definition name; the value of the `agent` tool's `type` parameter.
    pub name: String,
    /// One-line purpose shown in the `agent` tool description (the model
    /// picks a type from it) and in the UI.
    pub description: String,
    /// Kind-specific payload.
    pub kind: AgentKindDef,
    /// Markdown body. For local definitions this is the second prompt layer
    /// appended after the builtin skeleton (§4); for `kind: acp` it is the
    /// task-frame template prepended to the caller's task (§6). May be empty.
    pub body: String,
    /// Where the definition came from.
    pub source: DefinitionSource,
}

/// Errors produced by [`parse_agent_md`].
///
/// Every variant carries the definition's file stem so that directory
/// loaders can log-and-skip a broken file without losing which file it was.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AgentDefError {
    /// The file's first line is not the `---` frontmatter delimiter.
    #[error("agent definition `{stem}`: missing frontmatter (first line must be `---`)")]
    MissingFrontmatter {
        /// File stem (file name without `.md`) identifying the definition.
        stem: String,
    },

    /// The opening `---` has no closing `---` line.
    #[error("agent definition `{stem}`: unterminated frontmatter (no closing `---` line)")]
    UnterminatedFrontmatter {
        /// File stem (file name without `.md`) identifying the definition.
        stem: String,
    },

    /// YAML failure inside the frontmatter block: a syntax error, or the
    /// block is not a mapping of fields.
    #[error("agent definition `{stem}`: frontmatter YAML error: {message}")]
    Yaml {
        /// File stem (file name without `.md`) identifying the definition.
        stem: String,
        /// The parser or shape diagnostic.
        message: String,
    },

    /// A required field is absent or empty — `description` on any
    /// definition, or `command` on a `kind: acp` definition.
    #[error("agent definition `{stem}`: missing required field `{field}`")]
    MissingField {
        /// File stem (file name without `.md`) identifying the definition.
        stem: String,
        /// The required field's name.
        field: &'static str,
    },

    /// The frontmatter sets a field outside the documented set.
    #[error("agent definition `{stem}`: unknown field `{field}` (known fields: {known})")]
    UnknownField {
        /// File stem (file name without `.md`) identifying the definition.
        stem: String,
        /// The unrecognized field name.
        field: String,
        /// Comma-separated list of the recognized fields.
        known: String,
    },

    /// A known field has the wrong type, an out-of-range value, or is set on
    /// a kind it does not apply to (e.g. `command` on a local definition).
    #[error("agent definition `{stem}`: invalid field `{field}`: {message}")]
    Validation {
        /// File stem (file name without `.md`) identifying the definition.
        stem: String,
        /// The offending field's name.
        field: String,
        /// What is wrong with the field.
        message: String,
    },
}

impl AgentDefError {
    /// Builds an [`AgentDefError::Validation`] for `field` on `stem`.
    fn invalid(stem: &str, field: &str, message: impl Into<String>) -> Self {
        Self::Validation {
            stem: stem.to_owned(),
            field: field.to_owned(),
            message: message.into(),
        }
    }
}

/// Parses a markdown subagent-definition file (`docs/dyn-agents.md` §3.1).
///
/// Layout: the first line must be `---`; everything up to the next `---`
/// line is a YAML mapping of fields; everything after it is the markdown
/// body, kept verbatim apart from leading/trailing whitespace. The body may
/// be empty — a local definition may rely on the builtin skeleton alone.
///
/// Frontmatter fields:
///
/// - `name` — defaults to `file_stem` (the file name without `.md`).
/// - `description` — required; missing or empty is an error.
/// - `kind` — `local` (default) or `acp`.
/// - `tools` — local only; a YAML list or a comma-separated string.
/// - `model`, `max_steps` — local only.
/// - `command` — `acp` only and required there; a YAML list (argv form).
/// - `env` — `acp` only; a string-to-string map.
///
/// Unknown fields, and known fields set on a kind they do not apply to, are
/// errors: a misconfigured definition is never silently tolerated.
///
/// The returned definition's [`source`](AgentDefinition::source) defaults to
/// [`DefinitionSource::User`]; loaders parsing files from another source
/// overwrite the public field (registry assembly is the M2-2 follow-up).
pub fn parse_agent_md(file_stem: &str, content: &str) -> Result<AgentDefinition, AgentDefError> {
    // Tolerate a UTF-8 BOM from Windows editors.
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let (frontmatter, body) = split_frontmatter(file_stem, content)?;
    let fields = parse_frontmatter(file_stem, frontmatter)?;
    build_definition(file_stem, &fields, body)
}

/// Splits `content` into the raw YAML frontmatter block and the trimmed body.
///
/// The scan tracks byte offsets so the body slice keeps the original
/// markdown verbatim (only leading/trailing whitespace is trimmed).
fn split_frontmatter<'a>(
    stem: &str,
    content: &'a str,
) -> Result<(&'a str, &'a str), AgentDefError> {
    let (first, rest) = match content.find('\n') {
        Some(i) => (&content[..i], &content[i + 1..]),
        None => (content, ""),
    };
    if first.trim_end() != "---" {
        return Err(AgentDefError::MissingFrontmatter {
            stem: stem.to_owned(),
        });
    }
    let mut offset = 0;
    for chunk in rest.split_inclusive('\n') {
        let line = chunk.strip_suffix('\n').unwrap_or(chunk);
        // `trim_end` also absorbs a `\r` from CRLF line endings.
        if line.trim_end() == "---" {
            let yaml = &rest[..offset];
            let body = rest[offset + chunk.len()..].trim();
            return Ok((yaml, body));
        }
        offset += chunk.len();
    }
    Err(AgentDefError::UnterminatedFrontmatter {
        stem: stem.to_owned(),
    })
}

/// Owned view over the frontmatter mapping; explicit YAML nulls are kept but
/// read as absent by [`present`].
type RawFields = BTreeMap<String, serde_yml::Value>;

/// Parses the YAML block into a field map, rejecting non-mapping blocks,
/// non-string keys, and unknown fields.
fn parse_frontmatter(stem: &str, yaml: &str) -> Result<RawFields, AgentDefError> {
    if yaml.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let value: serde_yml::Value = serde_yml::from_str(yaml).map_err(|e| AgentDefError::Yaml {
        stem: stem.to_owned(),
        message: e.to_string(),
    })?;
    let serde_yml::Value::Mapping(mapping) = value else {
        if matches!(value, serde_yml::Value::Null) {
            // A comment-only block parses as a null document.
            return Ok(BTreeMap::new());
        }
        return Err(AgentDefError::Yaml {
            stem: stem.to_owned(),
            message: format!(
                "frontmatter must be a mapping of fields, got {}",
                yaml_kind(&value)
            ),
        });
    };
    let mut fields = BTreeMap::new();
    for (key, value) in &mapping {
        let serde_yml::Value::String(name) = key else {
            return Err(AgentDefError::invalid(
                stem,
                "<frontmatter>",
                format!("field names must be strings, got {}", yaml_kind(key)),
            ));
        };
        if !KNOWN_FIELDS.contains(&name.as_str()) {
            return Err(AgentDefError::UnknownField {
                stem: stem.to_owned(),
                field: name.clone(),
                known: KNOWN_FIELDS.join(", "),
            });
        }
        fields.insert(name.clone(), value.clone());
    }
    Ok(fields)
}

/// Returns the value set for `key`, treating an explicit YAML null as absent.
fn present<'a>(fields: &'a RawFields, key: &str) -> Option<&'a serde_yml::Value> {
    fields
        .get(key)
        .filter(|v| !matches!(v, serde_yml::Value::Null))
}

/// Reads an optional string field.
fn take_string(
    fields: &RawFields,
    stem: &str,
    key: &'static str,
) -> Result<Option<String>, AgentDefError> {
    match present(fields, key) {
        None => Ok(None),
        Some(serde_yml::Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(AgentDefError::invalid(
            stem,
            key,
            format!("expected a string, got {}", yaml_kind(other)),
        )),
    }
}

/// Reads `tools`: a YAML list of strings, or one comma-separated string.
fn take_tools(fields: &RawFields, stem: &str) -> Result<Option<Vec<String>>, AgentDefError> {
    match present(fields, "tools") {
        None => Ok(None),
        Some(serde_yml::Value::String(s)) => Ok(Some(
            s.split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
                .collect(),
        )),
        Some(serde_yml::Value::Sequence(items)) => Ok(Some(take_string_list(
            stem,
            "tools",
            items,
            "a list of strings",
        )?)),
        Some(other) => Err(AgentDefError::invalid(
            stem,
            "tools",
            format!(
                "expected a comma-separated string or a list of strings, got {}",
                yaml_kind(other)
            ),
        )),
    }
}

/// Reads `command`: a YAML list of strings (argv form).
fn take_command(fields: &RawFields, stem: &str) -> Result<Option<Vec<String>>, AgentDefError> {
    match present(fields, "command") {
        None => Ok(None),
        Some(serde_yml::Value::Sequence(items)) => Ok(Some(take_string_list(
            stem,
            "command",
            items,
            "a list of strings (argv form)",
        )?)),
        Some(other) => Err(AgentDefError::invalid(
            stem,
            "command",
            format!(
                "expected a list of strings (argv form), got {}",
                yaml_kind(other)
            ),
        )),
    }
}

/// Extracts a string list from a YAML sequence.
fn take_string_list(
    stem: &str,
    key: &'static str,
    items: &[serde_yml::Value],
    expected: &str,
) -> Result<Vec<String>, AgentDefError> {
    let mut list = Vec::with_capacity(items.len());
    for item in items {
        match item {
            serde_yml::Value::String(s) => list.push(s.clone()),
            other => {
                return Err(AgentDefError::invalid(
                    stem,
                    key,
                    format!("expected {expected}, got {} entry", yaml_kind(other)),
                ));
            }
        }
    }
    Ok(list)
}

/// Reads `max_steps`: a non-negative integer fitting `u32`.
fn take_max_steps(fields: &RawFields, stem: &str) -> Result<Option<u32>, AgentDefError> {
    match present(fields, "max_steps") {
        None => Ok(None),
        Some(serde_yml::Value::Number(n)) => {
            let raw = n.as_u64().ok_or_else(|| {
                AgentDefError::invalid(stem, "max_steps", "expected a non-negative integer")
            })?;
            let steps = u32::try_from(raw).map_err(|_| {
                AgentDefError::invalid(stem, "max_steps", format!("{raw} exceeds the u32 range"))
            })?;
            Ok(Some(steps))
        }
        Some(other) => Err(AgentDefError::invalid(
            stem,
            "max_steps",
            format!("expected a non-negative integer, got {}", yaml_kind(other)),
        )),
    }
}

/// Reads `env`: a string-to-string map.
fn take_env(
    fields: &RawFields,
    stem: &str,
) -> Result<Option<BTreeMap<String, String>>, AgentDefError> {
    match present(fields, "env") {
        None => Ok(None),
        Some(serde_yml::Value::Mapping(entries)) => {
            let mut env = BTreeMap::new();
            for (key, value) in entries {
                let serde_yml::Value::String(name) = key else {
                    return Err(AgentDefError::invalid(
                        stem,
                        "env",
                        format!("variable names must be strings, got {}", yaml_kind(key)),
                    ));
                };
                let serde_yml::Value::String(val) = value else {
                    return Err(AgentDefError::invalid(
                        stem,
                        "env",
                        format!(
                            "value for `{name}` must be a string, got {} (quote scalars)",
                            yaml_kind(value)
                        ),
                    ));
                };
                env.insert(name.clone(), val.clone());
            }
            Ok(Some(env))
        }
        Some(other) => Err(AgentDefError::invalid(
            stem,
            "env",
            format!("expected a string-to-string map, got {}", yaml_kind(other)),
        )),
    }
}

/// Validates the field combination and assembles the definition.
fn build_definition(
    stem: &str,
    fields: &RawFields,
    body: &str,
) -> Result<AgentDefinition, AgentDefError> {
    let name = match take_string(fields, stem, "name")? {
        Some(name) if name.trim().is_empty() => {
            return Err(AgentDefError::invalid(
                stem,
                "name",
                "must not be empty (omit the field to inherit the file stem)",
            ));
        }
        Some(name) => name,
        None => stem.to_owned(),
    };
    let description = take_string(fields, stem, "description")?
        .filter(|d| !d.trim().is_empty())
        .ok_or_else(|| AgentDefError::MissingField {
            stem: stem.to_owned(),
            field: "description",
        })?;
    let kind_raw = take_string(fields, stem, "kind")?;
    let model = take_string(fields, stem, "model")?;
    let tools = take_tools(fields, stem)?;
    let max_steps = take_max_steps(fields, stem)?;
    let command = take_command(fields, stem)?;
    let env = take_env(fields, stem)?;

    // Fields scoped to the other kind are rejected rather than silently
    // dropped: a misconfigured definition must never be silent.
    let kind = match kind_raw.as_deref() {
        None | Some("local") => {
            if command.is_some() {
                return Err(AgentDefError::invalid(
                    stem,
                    "command",
                    "only valid for `kind: acp` definitions",
                ));
            }
            if env.is_some() {
                return Err(AgentDefError::invalid(
                    stem,
                    "env",
                    "only valid for `kind: acp` definitions",
                ));
            }
            AgentKindDef::Local {
                model,
                tools,
                max_steps,
            }
        }
        Some("acp") => {
            if tools.is_some() {
                return Err(AgentDefError::invalid(
                    stem,
                    "tools",
                    "only valid for `kind: local` definitions",
                ));
            }
            if model.is_some() {
                return Err(AgentDefError::invalid(
                    stem,
                    "model",
                    "only valid for `kind: local` definitions",
                ));
            }
            if max_steps.is_some() {
                return Err(AgentDefError::invalid(
                    stem,
                    "max_steps",
                    "only valid for `kind: local` definitions",
                ));
            }
            let command =
                command
                    .filter(|c| !c.is_empty())
                    .ok_or_else(|| AgentDefError::MissingField {
                        stem: stem.to_owned(),
                        field: "command",
                    })?;
            AgentKindDef::ExternalAcp {
                command,
                env: env.unwrap_or_default(),
            }
        }
        Some(other) => {
            return Err(AgentDefError::invalid(
                stem,
                "kind",
                format!("unknown kind `{other}` (expected `local` or `acp`)"),
            ));
        }
    };

    Ok(AgentDefinition {
        name,
        description,
        kind,
        body: body.to_owned(),
        source: DefinitionSource::User,
    })
}

/// Human-readable name of a YAML value's kind, for error messages.
fn yaml_kind(value: &serde_yml::Value) -> &'static str {
    match value {
        serde_yml::Value::Null => "null",
        serde_yml::Value::Bool(_) => "a boolean",
        serde_yml::Value::Number(_) => "a number",
        serde_yml::Value::String(_) => "a string",
        serde_yml::Value::Sequence(_) => "a sequence",
        serde_yml::Value::Mapping(_) => "a mapping",
        serde_yml::Value::Tagged(_) => "a tagged value",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn local_full_fields_parse() {
        let content = r#"---
name: reviewer
description: Reviews code changes for correctness.
kind: local
tools:
  - read_file
  - grep
model: claude-haiku-4-5
max_steps: 12
---

You review code.
Report findings back.
"#;
        // The stem is ignored when `name` is set.
        let def = parse_agent_md("ignored_stem", content).unwrap();
        assert_eq!(def.name, "reviewer");
        assert_eq!(def.description, "Reviews code changes for correctness.");
        assert_eq!(
            def.kind,
            AgentKindDef::Local {
                model: Some("claude-haiku-4-5".to_owned()),
                tools: Some(strings(&["read_file", "grep"])),
                max_steps: Some(12),
            }
        );
        assert_eq!(def.body, "You review code.\nReport findings back.");
        assert_eq!(def.source, DefinitionSource::User);
    }

    #[test]
    fn local_minimal_fields_use_defaults() {
        let def = parse_agent_md("helper", "---\ndescription: Does things.\n---\n").unwrap();
        assert_eq!(def.name, "helper");
        assert_eq!(def.description, "Does things.");
        assert_eq!(
            def.kind,
            AgentKindDef::Local {
                model: None,
                tools: None,
                max_steps: None,
            }
        );
        assert_eq!(def.body, "");
    }

    #[test]
    fn acp_definition_parses_command_and_env() {
        let content = r#"---
name: peer
description: ACP peer agent.
kind: acp
command: ["peer-agent", "--acp"]
env:
  FOO: bar
  BAZ: qux
---

Task frame template.
"#;
        let def = parse_agent_md("peer", content).unwrap();
        assert_eq!(
            def.kind,
            AgentKindDef::ExternalAcp {
                command: strings(&["peer-agent", "--acp"]),
                env: BTreeMap::from([
                    ("FOO".to_owned(), "bar".to_owned()),
                    ("BAZ".to_owned(), "qux".to_owned()),
                ]),
            }
        );
        assert_eq!(def.body, "Task frame template.");

        // `env` is optional and defaults to an empty map.
        let def = parse_agent_md(
            "peer",
            "---\ndescription: ACP peer.\nkind: acp\ncommand: [peer-agent]\n---\n",
        )
        .unwrap();
        assert_eq!(
            def.kind,
            AgentKindDef::ExternalAcp {
                command: strings(&["peer-agent"]),
                env: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn name_defaults_to_file_stem() {
        let def = parse_agent_md("explorer", "---\ndescription: Explores.\n---\nbody").unwrap();
        assert_eq!(def.name, "explorer");
    }

    #[test]
    fn missing_description_is_an_error() {
        // Absent entirely.
        let err = parse_agent_md("x", "---\nname: x\n---\nbody").unwrap_err();
        assert!(matches!(
            err,
            AgentDefError::MissingField {
                field: "description",
                ..
            }
        ));
        // Explicit null.
        let err = parse_agent_md("x", "---\ndescription:\n---\n").unwrap_err();
        assert!(matches!(
            err,
            AgentDefError::MissingField {
                field: "description",
                ..
            }
        ));
        // Empty string.
        let err = parse_agent_md("x", "---\ndescription: \"\"\n---\n").unwrap_err();
        assert!(matches!(
            err,
            AgentDefError::MissingField {
                field: "description",
                ..
            }
        ));
    }

    #[test]
    fn missing_frontmatter_is_an_error() {
        let err = parse_agent_md("x", "no frontmatter here\njust text\n").unwrap_err();
        assert!(matches!(err, AgentDefError::MissingFrontmatter { .. }));
        assert!(err.to_string().contains('x'));
    }

    #[test]
    fn unterminated_frontmatter_is_an_error() {
        let err = parse_agent_md("x", "---\ndescription: x\n").unwrap_err();
        assert!(matches!(err, AgentDefError::UnterminatedFrontmatter { .. }));
    }

    #[test]
    fn unknown_field_is_an_error() {
        let err = parse_agent_md("x", "---\ndescription: x\nbogus: 1\n---\n").unwrap_err();
        assert!(matches!(err, AgentDefError::UnknownField { .. }));
        let message = err.to_string();
        assert!(message.contains("bogus"));
        assert!(message.contains("known fields:"));
    }

    #[test]
    fn tools_accept_yaml_list_or_comma_separated_string() {
        let from_list = parse_agent_md(
            "a",
            "---\ndescription: x\ntools:\n  - read_file\n  - grep\n---\n",
        )
        .unwrap();
        let from_flow_list =
            parse_agent_md("a", "---\ndescription: x\ntools: [read_file, grep]\n---\n").unwrap();
        let from_csv =
            parse_agent_md("a", "---\ndescription: x\ntools: read_file, grep\n---\n").unwrap();
        let expected = AgentKindDef::Local {
            model: None,
            tools: Some(strings(&["read_file", "grep"])),
            max_steps: None,
        };
        assert_eq!(from_list.kind, expected);
        assert_eq!(from_flow_list.kind, expected);
        assert_eq!(from_csv.kind, expected);
    }

    #[test]
    fn empty_body_after_frontmatter_is_empty_string() {
        let def = parse_agent_md("x", "---\ndescription: x\n---\n").unwrap();
        assert_eq!(def.body, "");
        // Whitespace-only body also collapses to the empty string.
        let def = parse_agent_md("x", "---\ndescription: x\n---\n\n   \n\n").unwrap();
        assert_eq!(def.body, "");
    }

    #[test]
    fn body_preserves_markdown_structure() {
        let content = "---\ndescription: x\n---\n# Heading\n\n- one\n- two\n\n```rust\nfn main() {}\n```\n\nTrailing paragraph.\n";
        let def = parse_agent_md("x", content).unwrap();
        assert_eq!(
            def.body,
            "# Heading\n\n- one\n- two\n\n```rust\nfn main() {}\n```\n\nTrailing paragraph."
        );
    }

    #[test]
    fn acp_without_command_is_an_error() {
        let err = parse_agent_md("p", "---\ndescription: x\nkind: acp\n---\n").unwrap_err();
        assert!(matches!(
            err,
            AgentDefError::MissingField {
                field: "command",
                ..
            }
        ));
        // An empty list counts as missing (argv needs at least the binary).
        let err =
            parse_agent_md("p", "---\ndescription: x\nkind: acp\ncommand: []\n---\n").unwrap_err();
        assert!(matches!(
            err,
            AgentDefError::MissingField {
                field: "command",
                ..
            }
        ));
    }

    #[test]
    fn kind_mismatched_fields_are_errors() {
        // `command` on a local definition.
        let err = parse_agent_md("x", "---\ndescription: x\ncommand: [foo]\n---\n").unwrap_err();
        assert!(matches!(err, AgentDefError::Validation { .. }));
        assert!(err.to_string().contains("`command`"));
        // `env` on a local definition.
        let err = parse_agent_md("x", "---\ndescription: x\nenv:\n  FOO: bar\n---\n").unwrap_err();
        assert!(err.to_string().contains("`env`"));
        // `tools` / `model` / `max_steps` on an acp definition.
        for content in [
            "---\ndescription: x\nkind: acp\ncommand: [foo]\ntools: [grep]\n---\n",
            "---\ndescription: x\nkind: acp\ncommand: [foo]\nmodel: m\n---\n",
            "---\ndescription: x\nkind: acp\ncommand: [foo]\nmax_steps: 3\n---\n",
        ] {
            let err = parse_agent_md("x", content).unwrap_err();
            assert!(matches!(err, AgentDefError::Validation { .. }));
            assert!(err.to_string().contains("only valid for `kind: local`"));
        }
    }

    #[test]
    fn unknown_kind_value_is_an_error() {
        let err = parse_agent_md("x", "---\ndescription: x\nkind: mcp\n---\n").unwrap_err();
        assert!(matches!(err, AgentDefError::Validation { .. }));
        assert!(err.to_string().contains("unknown kind `mcp`"));
    }

    #[test]
    fn invalid_field_types_are_errors() {
        // Negative step budget.
        let err = parse_agent_md("x", "---\ndescription: x\nmax_steps: -1\n---\n").unwrap_err();
        assert!(matches!(err, AgentDefError::Validation { .. }));
        // Non-string env value.
        let err = parse_agent_md(
            "x",
            "---\ndescription: x\nkind: acp\ncommand: [a]\nenv:\n  PORT: 8080\n---\n",
        )
        .unwrap_err();
        assert!(matches!(err, AgentDefError::Validation { .. }));
        // Non-string tools entry.
        let err = parse_agent_md("x", "---\ndescription: x\ntools: [1, 2]\n---\n").unwrap_err();
        assert!(matches!(err, AgentDefError::Validation { .. }));
    }

    #[test]
    fn malformed_yaml_is_an_error() {
        let err = parse_agent_md("x", "---\ndescription: [unclosed\n---\n").unwrap_err();
        assert!(matches!(err, AgentDefError::Yaml { .. }));
        // A non-mapping block (sequence) is a shape error, not fields.
        let err = parse_agent_md("x", "---\n- just\n- a list\n---\n").unwrap_err();
        assert!(matches!(err, AgentDefError::Yaml { .. }));
    }

    #[test]
    fn empty_or_comment_only_frontmatter_yields_all_defaults() {
        // Both forms leave every field at its default, so the required
        // `description` check is what fires.
        for content in ["---\n---\nbody", "---\n# nothing here\n---\nbody"] {
            let err = parse_agent_md("x", content).unwrap_err();
            assert!(matches!(
                err,
                AgentDefError::MissingField {
                    field: "description",
                    ..
                }
            ));
        }
    }

    #[test]
    fn crlf_line_endings_parse() {
        let def = parse_agent_md(
            "x",
            "---\r\ndescription: x\r\ntools: a, b\r\n---\r\n\r\nbody line\r\n",
        )
        .unwrap();
        assert_eq!(def.description, "x");
        assert_eq!(
            def.kind,
            AgentKindDef::Local {
                model: None,
                tools: Some(strings(&["a", "b"])),
                max_steps: None,
            }
        );
        assert_eq!(def.body, "body line");
    }
}
