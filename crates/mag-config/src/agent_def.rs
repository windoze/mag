//! Subagent definition model and markdown definition-file parsing.
//!
//! A subagent *definition* (`docs/dyn-agents.md` §3) is a declarative,
//! source-agnostic description of an agent type that the runtime `agent`
//! tool can instantiate. All provenances ([`DefinitionSource`]: builtin
//! constants, user markdown files under `~/.config/mag/agents/`, project
//! markdown files under `.mag/agents/`, TOML configuration entries) share the
//! one [`AgentDefinition`] model.
//!
//! This module implements the model, the markdown definition-file format
//! (§3.1: YAML frontmatter + markdown body) via [`parse_agent_md`], and the
//! registry assembly (§3.2): the builtin definitions, user/project directory
//! loading, the TOML projection, and the priority merge — all on
//! [`AgentDefinitionRegistry`].

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::snapshot::{ConfigSnapshot, ExternalAgentKind};

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

    /// Listing a definition directory failed. A directory that does not
    /// exist at all is *not* an error — it yields an empty registry.
    #[error("cannot list agent definition directory `{path}`: {source}")]
    Io {
        /// The directory that could not be listed.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
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

/// Body of the builtin `general-purpose` definition (§3.3): generic
/// task-execution guidance. It deliberately says nothing about the subagent
/// role or the report contract — that is the shared prompt skeleton's job
/// (§4, delivered in M3-3), and duplicating it here would drift.
const GENERAL_PURPOSE_BODY: &str = "\
Handle the task end to end: break it into concrete steps, gather the context
you need with the available tools, make the required changes, and check your
own results before finishing.

- Stay within the scope of the task; do not refactor, reformat, or \"improve\"
  unrelated code.
- Work from evidence — read files and probe with the tools instead of
  guessing; when an assumption proves wrong, say so and adjust.
- When something blocks completion, report the blocker plainly instead of
  working around it silently.";

/// Body of the builtin `explorer` definition (§3.3): read-only exploration
/// guidance, with the same skeleton separation as [`GENERAL_PURPOSE_BODY`].
const EXPLORER_BODY: &str = "\
Explore the codebase to answer the question or to locate what the task points
at. Your tool set is read-only: list directories, search for symbols and
patterns, and read the relevant code.

- Ground every claim in code you actually opened; never answer from memory
  or from naming alone.
- Cite precise locations (`path:line`) for each finding so the supervisor can
  navigate straight to the evidence.
- Separate what the code shows from what you infer, and flag questions you
  could not settle with read-only access.";

/// A name-keyed table of [`AgentDefinition`]s merged from the four
/// definition sources (`docs/dyn-agents.md` §3.2).
///
/// Source priority on a name clash is `Builtin < User < Project < Toml` (see
/// [`DefinitionSource`]). Assemble the table by folding
/// [`merge`](Self::merge) from the lowest-priority source upwards:
///
/// ```no_run
/// # fn assemble(
/// #     user_dir: &std::path::Path,
/// #     project_dir: &std::path::Path,
/// #     snapshot: &mag_config::ConfigSnapshot,
/// #     bound_agent: &str,
/// # ) -> Result<mag_config::AgentDefinitionRegistry, mag_config::AgentDefError> {
/// use mag_config::AgentDefinitionRegistry;
/// let registry = AgentDefinitionRegistry::merge(
///     AgentDefinitionRegistry::merge(
///         AgentDefinitionRegistry::merge(
///             AgentDefinitionRegistry::builtin(),
///             AgentDefinitionRegistry::load_user_dir(user_dir)?,
///         ),
///         AgentDefinitionRegistry::load_project_dir(project_dir)?,
///     ),
///     AgentDefinitionRegistry::from_toml_snapshot(snapshot, bound_agent),
/// );
/// # Ok(registry)
/// # }
/// ```
///
/// The table is ordered by name, so the [`describe_for_tool`] enumeration is
/// stable across runs.
///
/// [`describe_for_tool`]: Self::describe_for_tool
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentDefinitionRegistry {
    defs: BTreeMap<String, AgentDefinition>,
}

impl AgentDefinitionRegistry {
    /// The two builtin definitions (§3.3), both with
    /// [`DefinitionSource::Builtin`]:
    ///
    /// - `general-purpose` — the zero-configuration fallback and the default
    ///   for the `agent` tool's `type` parameter; inherits the supervisor's
    ///   model, tool surface, and step budget.
    /// - `explorer` — read-only codebase exploration, narrowed to
    ///   `read_file`, `list_dir`, and `grep` (the read-only subset of the
    ///   builtin tool set).
    #[must_use]
    pub fn builtin() -> Self {
        let defs = [
            AgentDefinition {
                name: "general-purpose".to_owned(),
                description: "General-purpose task execution; the default type when no \
                              specialized one fits."
                    .to_owned(),
                kind: AgentKindDef::Local {
                    model: None,
                    tools: None,
                    max_steps: None,
                },
                body: GENERAL_PURPOSE_BODY.to_owned(),
                source: DefinitionSource::Builtin,
            },
            AgentDefinition {
                name: "explorer".to_owned(),
                description: "Read-only code exploration: locating files, searching code, and \
                              answering codebase questions with precise file references."
                    .to_owned(),
                kind: AgentKindDef::Local {
                    model: None,
                    tools: Some(
                        ["read_file", "list_dir", "grep"]
                            .map(str::to_owned)
                            .to_vec(),
                    ),
                    max_steps: None,
                },
                body: EXPLORER_BODY.to_owned(),
                source: DefinitionSource::Builtin,
            },
        ];
        Self {
            defs: defs
                .into_iter()
                .map(|def| (def.name.clone(), def))
                .collect(),
        }
    }

    /// Loads user-level definitions (`*.md` files directly under `dir`,
    /// conventionally [`default_user_agents_dir`]).
    ///
    /// A missing directory yields an empty registry, not an error. A file
    /// that fails to parse is logged and skipped — one broken definition
    /// never takes the others down with it.
    pub fn load_user_dir(dir: &Path) -> Result<Self, AgentDefError> {
        Self::load_dir(dir, DefinitionSource::User)
    }

    /// Loads project-level definitions (`*.md` files directly under `dir`,
    /// conventionally [`project_agents_dir`]); same tolerance rules as
    /// [`load_user_dir`](Self::load_user_dir).
    pub fn load_project_dir(dir: &Path) -> Result<Self, AgentDefError> {
        Self::load_dir(dir, DefinitionSource::Project)
    }

    /// Shared directory loader behind [`load_user_dir`](Self::load_user_dir)
    /// and [`load_project_dir`](Self::load_project_dir); `source` overwrites
    /// the parser's default provenance on every parsed definition.
    fn load_dir(dir: &Path, source: DefinitionSource) -> Result<Self, AgentDefError> {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(AgentDefError::Io {
                    path: dir.display().to_string(),
                    source: error,
                });
            }
        };
        // Collect and sort so load order — and therefore which file wins a
        // same-directory name clash — is deterministic.
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry.path()),
                Err(error) => {
                    tracing::warn!(
                        dir = %dir.display(),
                        %error,
                        "skipping unreadable entry in agent definition directory"
                    );
                    None
                }
            })
            .filter(|path| path.extension().and_then(OsStr::to_str) == Some("md"))
            .filter(|path| path.is_file())
            .collect();
        paths.sort();

        let mut defs = BTreeMap::new();
        for path in paths {
            let stem = path
                .file_stem()
                .and_then(OsStr::to_str)
                .unwrap_or_default()
                .to_owned();
            let content = match std::fs::read_to_string(&path) {
                Ok(content) => content,
                Err(error) => {
                    tracing::warn!(
                        path = %path.display(),
                        %error,
                        "skipping unreadable agent definition file"
                    );
                    continue;
                }
            };
            match parse_agent_md(&stem, &content) {
                Ok(mut def) => {
                    def.source = source;
                    if defs.insert(def.name.clone(), def).is_some() {
                        tracing::warn!(
                            path = %path.display(),
                            "duplicate agent definition name in directory; \
                             the alphabetically later file wins"
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        path = %path.display(),
                        %error,
                        "skipping broken agent definition file"
                    );
                }
            }
        }
        Ok(Self { defs })
    }

    /// Projects the TOML configuration into definitions
    /// ([`DefinitionSource::Toml`], §3.2 source 4) — the mapping formerly
    /// applied by mag-core's session assembly:
    ///
    /// - every `[agents.<name>]` entry except the session-bound
    ///   `bound_agent` becomes a local definition: `role` (or the fallback
    ///   `Local subagent \`<name>\``) as the description, `system_prompt`
    ///   (or empty) as the body, `model`, the enabled tool-name set, and the
    ///   budget's `max_steps`;
    /// - every `[external_agents.<name>]` entry of kind `acp` becomes an
    ///   external definition (other kinds, and entries without a spawn
    ///   command, are logged and skipped).
    #[must_use]
    pub fn from_toml_snapshot(snapshot: &ConfigSnapshot, bound_agent: &str) -> Self {
        let mut defs = BTreeMap::new();
        // Iteration over the `BTreeMap`s keeps the projection deterministic.
        for agent in snapshot.agents().values() {
            if agent.name() == bound_agent {
                continue;
            }
            let tools = agent.tools_list().map(|tools| {
                tools
                    .iter()
                    .filter(|tool| tool.is_enabled())
                    .map(|tool| tool.name().to_owned())
                    .collect()
            });
            let max_steps = agent
                .budget()
                .and_then(|budget| budget.max_steps())
                .and_then(|steps| match u32::try_from(steps) {
                    Ok(steps) => Some(steps),
                    Err(_) => {
                        tracing::warn!(
                            agent = agent.name(),
                            steps,
                            "agent budget max_steps exceeds the u32 range; ignoring the override"
                        );
                        None
                    }
                });
            defs.insert(
                agent.name().to_owned(),
                AgentDefinition {
                    name: agent.name().to_owned(),
                    description: agent
                        .role()
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("Local subagent `{}`", agent.name())),
                    kind: AgentKindDef::Local {
                        model: agent.model().map(str::to_owned),
                        tools,
                        max_steps,
                    },
                    body: agent.system_prompt().map(str::to_owned).unwrap_or_default(),
                    source: DefinitionSource::Toml,
                },
            );
        }
        for external in snapshot.external_agents().values() {
            if !matches!(external.effective_kind(), ExternalAgentKind::Acp) {
                tracing::warn!(
                    agent = external.name(),
                    kind = %external.effective_kind(),
                    "skipping external agent definition of unsupported kind"
                );
                continue;
            }
            if external.command().is_empty() {
                tracing::warn!(
                    agent = external.name(),
                    "skipping external agent definition without a spawn command"
                );
                continue;
            }
            let name = external.name().to_owned();
            if defs.contains_key(&name) {
                tracing::warn!(
                    agent = %name,
                    "external agent shadows a same-named local agent definition from TOML"
                );
            }
            defs.insert(
                name.clone(),
                AgentDefinition {
                    name,
                    description: format!("External ACP subagent `{}`", external.name()),
                    kind: AgentKindDef::ExternalAcp {
                        command: external.command().to_vec(),
                        env: external.env().cloned().unwrap_or_default(),
                    },
                    body: String::new(),
                    source: DefinitionSource::Toml,
                },
            );
        }
        Self { defs }
    }

    /// Merges two registries: on a name clash the definition from `over`
    /// replaces the one from `base`. Chained from the lowest-priority source
    /// upwards this yields the §3.2 order `Builtin < User < Project < Toml`.
    #[must_use]
    pub fn merge(base: Self, over: Self) -> Self {
        let mut defs = base.defs;
        defs.extend(over.defs);
        Self { defs }
    }

    /// Looks up a definition by name (the `agent` tool's `type` parameter).
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&AgentDefinition> {
        self.defs.get(name)
    }

    /// The number of definitions in the table.
    #[must_use]
    pub fn len(&self) -> usize {
        self.defs.len()
    }

    /// Whether the table holds no definitions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// Enumerates every definition for the `agent` tool's description
    /// (§5.1: the model picks a `type` from this text). One line per
    /// definition, ordered by name; external definitions carry an `(acp)`
    /// marker so the model can tell them apart from local ones.
    #[must_use]
    pub fn describe_for_tool(&self) -> String {
        let mut lines = Vec::with_capacity(self.defs.len() + 1);
        lines.push("Available agent types:".to_owned());
        for def in self.defs.values() {
            let marker = match def.kind {
                AgentKindDef::Local { .. } => "",
                AgentKindDef::ExternalAcp { .. } => " (acp)",
            };
            lines.push(format!("- {}{marker}: {}", def.name, def.description));
        }
        lines.join("\n")
    }
}

/// The default user-level definition directory
/// (`docs/dyn-agents.md` §3.2 source 2): `$XDG_CONFIG_HOME/mag/agents` when
/// `XDG_CONFIG_HOME` is set, else `~/.config/mag/agents`; when neither
/// variable is available the relative `mag/agents` is used. This mirrors the
/// config-file convention (`crates/mag/src/main.rs` `default_config_path`).
#[must_use]
pub fn default_user_agents_dir() -> PathBuf {
    user_agents_dir_from(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// The pure core of [`default_user_agents_dir`], with the environment lookup
/// injected so the precedence rules are testable without mutating process
/// state. Empty values count as unset.
fn user_agents_dir_from(xdg: Option<&OsStr>, home: Option<&OsStr>) -> PathBuf {
    if let Some(xdg) = xdg.filter(|v| !v.is_empty()) {
        return PathBuf::from(xdg).join("mag").join("agents");
    }
    if let Some(home) = home.filter(|v| !v.is_empty()) {
        return PathBuf::from(home)
            .join(".config")
            .join("mag")
            .join("agents");
    }
    PathBuf::from("mag").join("agents")
}

/// The project-level definition directory for a session rooted at `cwd`
/// (`docs/dyn-agents.md` §3.2 source 3): `<cwd>/.mag/agents`.
#[must_use]
pub fn project_agents_dir(cwd: &Path) -> PathBuf {
    cwd.join(".mag").join("agents")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ConfigDto;

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

    // ---- M2-2: registry assembly ----

    /// The `docs/CLI.md` §4.2 example, verbatim (mirrors
    /// `tests/roundtrip.rs`), used for the TOML projection tests.
    const EXAMPLE_TOML: &str = r#"
# ~/.config/mag/config.toml（DTO 的 TOML 形态）
[providers.anthropic]
wire = "anthropic"
base_url = "https://api.anthropic.com"
api_key = { env = "ANTHROPIC_API_KEY" }     # secret 引用，不落盘

[providers.local_proxy]
wire = "openai"
base_url = "http://127.0.0.1:8317"
api_key = { keyring = "mag/local_proxy" }

[agents.default]
provider = "anthropic"
model = "claude-sonnet-4-5"
tools = ["read_file", "list_dir", "grep", "shell", "ask_user"]

[agents.reviewer]
provider = "local_proxy"
model = "gpt-5-codex"
tools = ["read_file", "grep"]

[external_agents.peer_acp]                  # external ACP agent 来源（决策 D3）
kind = "acp"
command = ["peer-agent", "--acp"]           # spawn 命令行；工作目录隔离由 agent-lib 负责

[tools.shell]
approval = "ask"                            # ask | allow | deny（→ ApprovalPolicy 映射）

[session]
routing = "model_routed"
budget = { max_tokens = 200000 }
"#;

    fn example_snapshot() -> ConfigSnapshot {
        let dto = ConfigDto::parse_str(EXAMPLE_TOML).expect("§4.2 example must parse");
        ConfigSnapshot::resolve(&dto, 1).expect("§4.2 example must resolve")
    }

    fn write_file(dir: &Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).expect("write definition file");
    }

    fn md(description: &str, extra: &str) -> String {
        format!("---\ndescription: {description}\n{extra}---\nbody\n")
    }

    #[test]
    fn builtin_registry_contains_general_purpose_and_explorer() {
        let registry = AgentDefinitionRegistry::builtin();
        assert_eq!(registry.len(), 2);

        let general = registry.get("general-purpose").expect("general-purpose");
        assert_eq!(general.source, DefinitionSource::Builtin);
        assert!(!general.description.is_empty());
        assert!(!general.body.is_empty());
        // The fallback type inherits everything from the supervisor.
        assert_eq!(
            general.kind,
            AgentKindDef::Local {
                model: None,
                tools: None,
                max_steps: None,
            }
        );

        let explorer = registry.get("explorer").expect("explorer");
        assert_eq!(explorer.source, DefinitionSource::Builtin);
        assert!(!explorer.description.is_empty());
        assert!(!explorer.body.is_empty());
        // The read-only subset of the builtin tool set.
        assert_eq!(
            explorer.kind,
            AgentKindDef::Local {
                model: None,
                tools: Some(strings(&["read_file", "list_dir", "grep"])),
                max_steps: None,
            }
        );
    }

    #[test]
    fn four_source_merge_priority_overrides_by_name() {
        // User directory: overrides the builtin `explorer`, shares
        // `shared`/`contested` with higher-priority sources.
        let user = tempfile::tempdir().expect("user tempdir");
        write_file(
            user.path(),
            "explorer.md",
            &md("user explorer override", ""),
        );
        write_file(
            user.path(),
            "shared.md",
            &md("user shared", "model: user-model\n"),
        );
        write_file(user.path(), "contested.md", &md("user contested", ""));
        write_file(user.path(), "user_only.md", &md("user only", ""));

        // Project directory: same shape, higher priority.
        let project = tempfile::tempdir().expect("project tempdir");
        write_file(
            project.path(),
            "shared.md",
            &md("project shared", "max_steps: 7\n"),
        );
        write_file(project.path(), "contested.md", &md("project contested", ""));
        write_file(project.path(), "project_only.md", &md("project only", ""));

        // TOML: highest priority, claims `shared`.
        let snapshot = ConfigSnapshot::resolve(
            &ConfigDto::parse_str("[agents.shared]\nrole = \"toml shared\"\n").expect("parse toml"),
            1,
        )
        .expect("resolve toml");

        let registry = AgentDefinitionRegistry::merge(
            AgentDefinitionRegistry::merge(
                AgentDefinitionRegistry::merge(
                    AgentDefinitionRegistry::builtin(),
                    AgentDefinitionRegistry::load_user_dir(user.path()).expect("load user dir"),
                ),
                AgentDefinitionRegistry::load_project_dir(project.path())
                    .expect("load project dir"),
            ),
            AgentDefinitionRegistry::from_toml_snapshot(&snapshot, "default"),
        );

        // toml > project > user > builtin on `shared`.
        let shared = registry.get("shared").expect("shared");
        assert_eq!(shared.source, DefinitionSource::Toml);
        assert_eq!(shared.description, "toml shared");
        // project > user on `contested`.
        let contested = registry.get("contested").expect("contested");
        assert_eq!(contested.source, DefinitionSource::Project);
        assert_eq!(contested.description, "project contested");
        // user > builtin on `explorer`.
        let explorer = registry.get("explorer").expect("explorer");
        assert_eq!(explorer.source, DefinitionSource::User);
        assert_eq!(explorer.description, "user explorer override");
        // Uncontested definitions from every source survive.
        assert_eq!(
            registry.get("user_only").expect("user_only").source,
            DefinitionSource::User
        );
        assert_eq!(
            registry.get("project_only").expect("project_only").source,
            DefinitionSource::Project
        );
        assert_eq!(
            registry
                .get("general-purpose")
                .expect("general-purpose")
                .source,
            DefinitionSource::Builtin
        );
        assert_eq!(registry.len(), 6);
    }

    #[test]
    fn missing_and_empty_directories_load_empty() {
        let base = tempfile::tempdir().expect("tempdir");
        let missing = AgentDefinitionRegistry::load_user_dir(&base.path().join("does-not-exist"))
            .expect("missing directory is not an error");
        assert!(missing.is_empty());

        let empty = tempfile::tempdir().expect("empty tempdir");
        let registry =
            AgentDefinitionRegistry::load_project_dir(empty.path()).expect("empty directory loads");
        assert_eq!(registry, AgentDefinitionRegistry::default());
    }

    #[test]
    fn broken_markdown_is_skipped_not_fatal() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Sorts before `good.md`: the failure must not abort the load.
        write_file(dir.path(), "bad.md", "no frontmatter at all\n");
        write_file(dir.path(), "good.md", &md("loads fine", ""));
        // Non-`.md` files are ignored, and so are directories named `*.md`.
        write_file(dir.path(), "notes.txt", "not a definition\n");
        std::fs::create_dir(dir.path().join("traps.md")).expect("create decoy directory");

        let registry = AgentDefinitionRegistry::load_user_dir(dir.path()).expect("load dir");
        assert_eq!(registry.len(), 1);
        let good = registry.get("good").expect("good definition survived");
        assert_eq!(good.source, DefinitionSource::User);
        assert_eq!(good.description, "loads fine");
    }

    #[test]
    fn toml_projection_matches_cli_example() {
        let registry = AgentDefinitionRegistry::from_toml_snapshot(&example_snapshot(), "default");
        // Exactly two definitions: the bound `default` entry is excluded.
        assert_eq!(registry.len(), 2);

        let reviewer = registry.get("reviewer").expect("reviewer");
        assert_eq!(reviewer.source, DefinitionSource::Toml);
        // No `role` in the example: the fallback description kicks in.
        assert_eq!(reviewer.description, "Local subagent `reviewer`");
        // No `system_prompt`: empty body.
        assert_eq!(reviewer.body, "");
        assert_eq!(
            reviewer.kind,
            AgentKindDef::Local {
                model: Some("gpt-5-codex".to_owned()),
                tools: Some(strings(&["read_file", "grep"])),
                max_steps: None,
            }
        );

        let peer = registry.get("peer_acp").expect("peer_acp");
        assert_eq!(peer.source, DefinitionSource::Toml);
        assert_eq!(peer.description, "External ACP subagent `peer_acp`");
        assert_eq!(peer.body, "");
        assert_eq!(
            peer.kind,
            AgentKindDef::ExternalAcp {
                command: strings(&["peer-agent", "--acp"]),
                env: BTreeMap::new(),
            }
        );

        assert!(registry.get("default").is_none());
    }

    #[test]
    fn toml_projection_maps_role_prompt_model_tools_and_budget() {
        let dto = ConfigDto::parse_str(
            r#"
[agents.default]
model = "sup"

[agents.helper]
role = "Helps out."
system_prompt = "You help."
model = "m1"
tools = ["grep", "read_file"]
budget = { max_steps = 9 }

[tools.grep]
enabled = false
"#,
        )
        .expect("parse toml");
        let snapshot = ConfigSnapshot::resolve(&dto, 1).expect("resolve toml");

        let registry = AgentDefinitionRegistry::from_toml_snapshot(&snapshot, "default");
        assert_eq!(registry.len(), 1);
        let helper = registry.get("helper").expect("helper");
        assert_eq!(helper.description, "Helps out.");
        assert_eq!(helper.body, "You help.");
        // The disabled tool drops out of the enabled name set.
        assert_eq!(
            helper.kind,
            AgentKindDef::Local {
                model: Some("m1".to_owned()),
                tools: Some(strings(&["read_file"])),
                max_steps: Some(9),
            }
        );

        // Binding a different agent changes which entry is excluded.
        let registry = AgentDefinitionRegistry::from_toml_snapshot(&snapshot, "helper");
        assert!(registry.get("helper").is_none());
        assert!(registry.get("default").is_some());
    }

    #[test]
    fn toml_projection_out_of_range_max_steps_is_ignored() {
        // `ResolvedAgent.budget.max_steps` is a `u64` but the definition model
        // is `u32`: an out-of-range override warns and falls back to unset
        // instead of silently truncating.
        let dto = ConfigDto::parse_str("[agents.huge]\nbudget = { max_steps = 5000000000 }\n")
            .expect("parse toml");
        let snapshot = ConfigSnapshot::resolve(&dto, 1).expect("resolve toml");
        let registry = AgentDefinitionRegistry::from_toml_snapshot(&snapshot, "default");
        let huge = registry.get("huge").expect("huge");
        assert_eq!(
            huge.kind,
            AgentKindDef::Local {
                model: None,
                tools: None,
                max_steps: None,
            }
        );
    }

    #[test]
    fn toml_projection_skips_external_without_command() {
        let dto =
            ConfigDto::parse_str("[external_agents.broken]\nkind = \"acp\"\n").expect("parse toml");
        let snapshot = ConfigSnapshot::resolve(&dto, 1).expect("resolve toml");
        let registry = AgentDefinitionRegistry::from_toml_snapshot(&snapshot, "default");
        assert!(registry.is_empty());
    }

    #[test]
    fn describe_for_tool_lists_all_names_and_descriptions() {
        let registry = AgentDefinitionRegistry::merge(
            AgentDefinitionRegistry::builtin(),
            AgentDefinitionRegistry::from_toml_snapshot(&example_snapshot(), "default"),
        );
        let text = registry.describe_for_tool();
        assert!(text.starts_with("Available agent types:"));

        let lines: Vec<&str> = text.lines().skip(1).collect();
        assert_eq!(lines.len(), 4);
        // Alphabetical by name, one line per definition, description included.
        for def in [
            registry.get("explorer").unwrap(),
            registry.get("general-purpose").unwrap(),
            registry.get("peer_acp").unwrap(),
            registry.get("reviewer").unwrap(),
        ] {
            let line = lines
                .iter()
                .find(|line| line.contains(def.name.as_str()))
                .unwrap_or_else(|| panic!("missing line for `{}`", def.name));
            assert!(line.contains(def.description.as_str()));
        }
        // External definitions carry the `(acp)` marker, local ones do not.
        let peer_line = lines.iter().find(|l| l.contains("peer_acp")).unwrap();
        assert!(peer_line.contains("(acp)"));
        let reviewer_line = lines.iter().find(|l| l.contains("reviewer")).unwrap();
        assert!(!reviewer_line.contains("(acp)"));
        // Stable ordering for the tool description.
        assert!(lines.is_sorted());
    }

    #[test]
    fn user_agents_dir_prefers_xdg_then_home() {
        let xdg = OsStr::new("/xdg");
        let home = OsStr::new("/home");
        assert_eq!(
            user_agents_dir_from(Some(xdg), Some(home)),
            PathBuf::from("/xdg/mag/agents")
        );
        assert_eq!(
            user_agents_dir_from(None, Some(home)),
            PathBuf::from("/home/.config/mag/agents")
        );
        // Empty values count as unset.
        assert_eq!(
            user_agents_dir_from(Some(OsStr::new("")), Some(home)),
            PathBuf::from("/home/.config/mag/agents")
        );
        assert_eq!(
            user_agents_dir_from(None, None),
            PathBuf::from("mag").join("agents")
        );
    }

    #[test]
    fn project_agents_dir_is_cwd_dot_mag_agents() {
        assert_eq!(
            project_agents_dir(Path::new("/repo")),
            PathBuf::from("/repo/.mag/agents")
        );
    }
}
