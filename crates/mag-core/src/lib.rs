#![warn(missing_docs)]

//! Transport-neutral engine core for mag.

mod assembly;
mod config;
mod driver;
mod engine;
mod event_bus;
mod history;
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
