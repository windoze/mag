//! TOML I/O and structural validation for [`ConfigDto`].
//!
//! - [`ConfigDto::parse_str`] / [`ConfigDto::to_string_pretty`] — pure serde;
//!   parse failures map to [`ConfigError::Parse`] with 1-based line/column
//!   derived from the parser's byte span.
//! - [`ConfigDto::load`] — read + parse + [`ConfigDto::validate`].
//! - [`ConfigDto::save_atomic`] — serialize to a sibling temp file, fsync,
//!   then `rename` over the target, so a file watcher never observes a
//!   half-written config (`docs/CLI.md` §4.2 write-through requirement).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{ConfigDto, ConfigError};

impl ConfigDto {
    /// Parses a TOML document into a [`ConfigDto`].
    ///
    /// This is pure deserialization: unknown sections/fields are ignored and
    /// any partial configuration is accepted. Structural validation is a
    /// separate step ([`ConfigDto::validate`]); semantic validation (dangling
    /// cross references, enum values) belongs to the resolve layer.
    pub fn parse_str(input: &str) -> Result<Self, ConfigError> {
        toml::from_str(input).map_err(|e| parse_error(input, None, &e))
    }

    /// Serializes the DTO to a pretty-printed TOML document.
    ///
    /// Output is deterministic: name-keyed maps are `BTreeMap`s, so sections
    /// are emitted in sorted order. Secret references keep their reference
    /// form (`{ env = "VAR" }` / `{ keyring = "NAME" }`); values are never
    /// materialized.
    pub fn to_string_pretty(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(|e| ConfigError::Serialize {
            message: e.to_string(),
        })
    }

    /// Reads, parses, and structurally validates a config file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let dto: Self =
            toml::from_str(&text).map_err(|e| parse_error(&text, Some(path.to_path_buf()), &e))?;
        dto.validate()?;
        Ok(dto)
    }

    /// Atomically writes the DTO as TOML to `path`.
    ///
    /// The document is first written to a sibling temp file
    /// (`<name>.tmp-<pid>-<counter>`), fsynced, then renamed over the target, so
    /// concurrent readers and file watchers see either the old or the new
    /// file, never a truncated one. Parent directories are created if
    /// missing.
    pub fn save_atomic(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        let path = path.as_ref();
        let text = self.to_string_pretty()?;

        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
        let tmp: PathBuf = path.with_file_name(format!(
            ".{}.tmp-{}-{}",
            file_name.as_deref().unwrap_or("config"),
            std::process::id(),
            TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));

        let write_result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            use std::io::Write as _;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            Ok::<(), std::io::Error>(())
        })();
        if let Err(source) = write_result {
            let _ = fs::remove_file(&tmp);
            return Err(ConfigError::Io { path: tmp, source });
        }

        fs::rename(&tmp, path).map_err(|source| {
            let _ = fs::remove_file(&tmp);
            ConfigError::Io {
                path: path.to_path_buf(),
                source,
            }
        })
    }

    /// Cheap structural validation with field-path errors.
    ///
    /// Checks invariants that make a parsed config trustworthy for the
    /// resolve layer: non-empty section names, non-empty strings where an
    /// empty one is meaningless, no duplicate tool names in an agent's tool
    /// list, well-formed spawn commands, and positive budget/timeout values.
    ///
    /// Deliberately **not** checked here (resolve-layer concerns, DTO→DO):
    /// existence of cross references, and the legality of enum-like strings
    /// (`wire`, `kind`, `approval`, `routing`).
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (name, provider) in &self.providers {
            check_name("providers", name)?;
            if let Some(wire) = &provider.wire {
                check_non_empty(format!("providers.{name}.wire"), wire)?;
            }
            if let Some(base_url) = &provider.base_url {
                check_non_empty(format!("providers.{name}.base_url"), base_url)?;
            }
        }

        for (name, agent) in &self.agents {
            check_name("agents", name)?;
            if let Some(provider) = &agent.provider {
                check_non_empty(format!("agents.{name}.provider"), provider)?;
            }
            if let Some(model) = &agent.model {
                check_non_empty(format!("agents.{name}.model"), model)?;
            }
            if let Some(tools) = &agent.tools {
                for (i, tool) in tools.iter().enumerate() {
                    check_non_empty(format!("agents.{name}.tools[{i}]"), tool)?;
                    if tools[..i].contains(tool) {
                        return Err(ConfigError::validation(
                            format!("agents.{name}.tools[{i}]"),
                            format!("duplicate tool name {tool:?}"),
                        ));
                    }
                }
            }
            if let Some(budget) = &agent.budget {
                check_budget(format!("agents.{name}.budget"), budget)?;
            }
        }

        for (name, external) in &self.external_agents {
            check_name("external_agents", name)?;
            if let Some(kind) = &external.kind {
                check_non_empty(format!("external_agents.{name}.kind"), kind)?;
            }
            if let Some(command) = &external.command {
                if command.is_empty() {
                    return Err(ConfigError::validation(
                        format!("external_agents.{name}.command"),
                        "command must not be an empty argv",
                    ));
                }
                for (i, arg) in command.iter().enumerate() {
                    check_non_empty(format!("external_agents.{name}.command[{i}]"), arg)?;
                }
            }
            if let Some(env) = &external.env {
                for key in env.keys() {
                    check_non_empty(format!("external_agents.{name}.env"), key)?;
                }
            }
        }

        for (name, tool) in &self.tools {
            check_name("tools", name)?;
            if let Some(approval) = &tool.approval {
                check_non_empty(format!("tools.{name}.approval"), approval)?;
            }
        }

        if let Some(session) = &self.session
            && let Some(budget) = &session.budget
        {
            check_budget("session.budget".to_string(), budget)?;
        }

        if let Some(approval) = &self.approval {
            if let Some(policy) = &approval.default_policy {
                check_non_empty("approval.default_policy".to_string(), policy)?;
            }
            if let Some(timeout) = approval.timeout_secs
                && timeout == 0
            {
                return Err(ConfigError::validation(
                    "approval.timeout_secs",
                    "timeout must be greater than 0",
                ));
            }
        }

        Ok(())
    }
}

/// Maps a `toml::de::Error` to [`ConfigError::Parse`], converting the byte
/// span into 1-based line/column.
fn parse_error(input: &str, path: Option<PathBuf>, error: &toml::de::Error) -> ConfigError {
    let (line, col) = error
        .span()
        .map(|span| line_col(input, span.start))
        .unwrap_or((0, 0));
    ConfigError::Parse {
        path,
        line,
        col,
        message: error.message().to_string(),
    }
}

/// Converts a byte offset into a 1-based (line, column) pair.
fn line_col(input: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in input.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

fn check_name(section: &str, name: &str) -> Result<(), ConfigError> {
    if name.trim().is_empty() {
        return Err(ConfigError::validation(
            section,
            "section name must not be empty",
        ));
    }
    Ok(())
}

fn check_non_empty(path: String, value: &str) -> Result<(), ConfigError> {
    if value.trim().is_empty() {
        return Err(ConfigError::validation(path, "value must not be empty"));
    }
    Ok(())
}

fn check_budget(path: String, budget: &crate::BudgetDto) -> Result<(), ConfigError> {
    for (field, value) in [
        ("max_steps", budget.max_steps),
        ("max_tokens", budget.max_tokens),
        ("max_cost_micros", budget.max_cost_micros),
        ("max_wall_time_secs", budget.max_wall_time_secs),
    ] {
        if let Some(v) = value
            && v == 0
        {
            return Err(ConfigError::validation(
                format!("{path}.{field}"),
                "budget limit must be greater than 0",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_tracks_newlines() {
        let input = "ab\ncde\nf";
        assert_eq!(line_col(input, 0), (1, 1));
        assert_eq!(line_col(input, 2), (1, 3));
        assert_eq!(line_col(input, 3), (2, 1));
        assert_eq!(line_col(input, 7), (3, 1));
    }
}
