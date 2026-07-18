#![warn(missing_docs)]

//! Transport-neutral engine core for mag.

mod engine;
mod event_bus;
mod ids;
mod llm;

pub use engine::{CommandOutput, Engine, EngineError, SessionInfo};
pub use event_bus::{EventBus, EventStream};
pub use ids::MagIds;
pub use llm::StreamingTapHandler;
