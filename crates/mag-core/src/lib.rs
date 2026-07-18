#![warn(missing_docs)]

//! Transport-neutral engine core for mag.

mod driver;
mod engine;
mod event_bus;

#[cfg(test)]
mod test_support;

pub use engine::{CommandOutput, Engine, EngineError, SessionInfo};
pub use event_bus::{EventBus, EventStream};
