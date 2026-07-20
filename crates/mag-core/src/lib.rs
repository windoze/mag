#![warn(missing_docs)]

//! Transport-neutral engine core for mag.

mod config;
mod driver;
mod engine;
mod event_bus;
mod persistence;
mod session;

#[cfg(test)]
mod test_support;

pub use config::{ConfigChange, ConfigService};
pub use engine::Engine;
pub use event_bus::{EventBus, EventStream};
pub use persistence::PersistenceError;
