#![warn(missing_docs)]

//! Transport-neutral engine core for mag.

mod assembly;
mod config;
mod driver;
mod engine;
mod event_bus;
mod history;
// Skeleton consumed by the M3-3/M3-4 tool handlers and drive tasks; the
// allowance is removed once those land (TODO.md M3-2).
#[allow(dead_code)]
mod instances;
mod persistence;
mod session;
mod turn_complete;

#[cfg(test)]
mod test_support;

pub use assembly::EngineError;
pub use config::{ConfigChange, ConfigService};
pub use engine::Engine;
pub use event_bus::{EventBus, EventStream};
pub use persistence::PersistenceError;
pub use turn_complete::{TurnCompleteListener, TurnCompletion, TurnSummary};
