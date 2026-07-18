#![warn(missing_docs)]

//! Transport-neutral engine core for mag.

mod driver;
mod engine;
mod event_bus;
mod session;

#[cfg(test)]
mod test_support;

pub use engine::Engine;
pub use event_bus::{EventBus, EventStream};
