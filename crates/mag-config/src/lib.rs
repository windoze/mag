#![warn(missing_docs)]

//! Runtime configuration DTOs, TOML I/O, and structural validation for mag.
//!
//! This crate is the home of the **DTO layer** of mag's runtime configuration
//! system (`docs/CLI.md` §4, decisions D2/D4). Following decision D4, the DTO
//! is a serde mirror of *any* configuration source — the TOML file at
//! `~/.config/mag/config.toml` is just one persistence form; the same types
//! also serve GUI settings panels, environment-derived overlays, and test
//! fixtures. The TOML field layout follows the `docs/CLI.md` §4.2 example,
//! which is the minimal complete configuration surface.
//!
//! # Layering
//!
//! - **DTO**: pure serde data + TOML read/write + structural validation.
//!   Secret fields hold *references* only ([`SecretRef`]); values are never
//!   materialized here.
//! - **DO** ([`ConfigSnapshot`], an `Arc` object graph): built from the DTO by
//!   [`ConfigSnapshot::resolve`] (validation + normalization: enum checks,
//!   cross-reference resolution into shared `Arc` nodes) and written back by
//!   [`ConfigSnapshot::project`] (lossless; secret references stay
//!   references). Snapshots are immutable and cheap to clone, so a session
//!   pinning one is unaffected by later updates (snapshot isolation,
//!   `docs/CLI.md` §4.4, decision D2).
//!
//! # Dependency boundary (hard constraint)
//!
//! `mag-config` is a pure data crate: it depends only on generic crates
//! (`serde` / `serde_yml` / `toml` / `thiserror`). It must **never** depend
//! on `agent-lib`, `mag-core`, or `mag-service`. Contract consumers (e.g.
//! `mag-service` config methods) depend on this crate, not the other way
//! around.
//!
//! # Entry points
//!
//! - [`ConfigDto::parse_str`] / [`ConfigDto::to_string_pretty`] — string-level
//!   TOML serde.
//! - [`ConfigDto::load`] / [`ConfigDto::save_atomic`] — file-level I/O; saving
//!   is atomic (temp file + `rename`) so file watchers never observe a
//!   half-written config.
//! - [`ConfigDto::validate`] — cheap structural validation with field-path
//!   errors ([`ConfigError::Validation`]). Parse failures carry line/column
//!   information ([`ConfigError::Parse`]).
//! - [`ConfigSnapshot::resolve`] / [`ConfigSnapshot::project`] — the two
//!   halves of the DTO↔DO conversion (`docs/CLI.md` §4.2, decision D4).
//! - [`parse_agent_md`] — markdown subagent-definition files
//!   (`docs/dyn-agents.md` §3.1): frontmatter parsing into the unified
//!   [`AgentDefinition`] model.

mod agent_def;
mod dto;
mod error;
mod io;
mod secret;
mod snapshot;

pub use agent_def::{
    AgentDefError, AgentDefinition, AgentKindDef, DefinitionSource, parse_agent_md,
};
pub use dto::{
    AgentDto, ApprovalSectionDto, BudgetDto, ConfigDto, ExternalAgentDto, ProviderDto,
    SessionDefaultsDto, ToolDto,
};
pub use error::ConfigError;
pub use secret::SecretRef;
pub use snapshot::{
    ApprovalConfig, ApprovalPolicyKind, Budget, ConfigSnapshot, ExternalAgentKind, ProviderWire,
    ResolvedAgent, ResolvedExternalAgent, ResolvedProvider, ResolvedTool, RoutingModeKind,
    SessionDefaults,
};
