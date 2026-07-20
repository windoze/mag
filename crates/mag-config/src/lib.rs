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
//! - **DTO** (this task's scope): pure serde data + TOML read/write +
//!   structural validation. Secret fields hold *references* only
//!   ([`SecretRef`]); values are never materialized here.
//! - **DO** (`ConfigSnapshot`, an `Arc` object graph): built from the DTO by
//!   the resolve half of the DTO↔DO conversion. That conversion — including
//!   cross-reference checks (agent → provider/tools names must exist) and
//!   enum validation (`approval` / `routing` values) — lives in this crate
//!   too but is a separate layer on top of these types.
//!
//! # Dependency boundary (hard constraint)
//!
//! `mag-config` is a pure data crate: it depends only on generic crates
//! (`serde` / `toml` / `thiserror`). It must **never** depend on `agent-lib`,
//! `mag-core`, or `mag-service`. Contract consumers (e.g. `mag-service`
//! config methods) depend on this crate, not the other way around.
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

mod dto;
mod error;
mod io;
mod secret;

pub use dto::{
    AgentDto, ApprovalSectionDto, BudgetDto, ConfigDto, ExternalAgentDto, ProviderDto,
    SessionDefaultsDto, ToolDto,
};
pub use error::ConfigError;
pub use secret::SecretRef;
