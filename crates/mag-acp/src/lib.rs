#![warn(missing_docs)]

//! `mag-acp`: the ACP (Agent Client Protocol) agent-side interface for mag.
//!
//! This crate is a thin, IO-free-at-the-core protocol translator that exposes the
//! frozen [`mag_service::MagService`] contract to ACP clients (e.g. Zed). It only
//! depends on `mag-service` and the `agent-client-protocol` crate, and it faces
//! the service exclusively through `Arc<dyn MagService>`.
//!
//! The design of record is [`docs/ACP.md`](../docs/ACP.md); the phase plan lives
//! in [`PLAN.md`](../PLAN.md) and the task list in [`TODO.md`](../TODO.md).
//!
//! This milestone (M1-1) lands the crate skeleton plus the pure-function [`map`]
//! module. Transport wiring and request handlers arrive in later milestones.

pub mod map;
