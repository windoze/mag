//! Dedicated run executor that owns each session's facade [`Agent`].
//!
//! agent-lib's facade run stream ([`Agent::stream`](agent_lib::facade::Agent::stream))
//! borrows the agent mutably and is **not** `Send`, so a run cannot be driven
//! inside a `Send` [`MagService`](mag_service::MagService) future or moved onto a
//! multi-threaded executor. The engine therefore hands runs to a single
//! background thread that hosts its own current-thread Tokio runtime and owns the
//! per-session [`SessionDriver`]s; the thread drives each run in place while the
//! engine's async methods only exchange `Send` channel messages with it.
//!
//! This is the minimal per-session run substrate required by
//! `impl MagService for Engine` (`docs/DESIGN.md` §3.0/§3.1). Milestone C2 turns
//! it into per-session actors with a cancellation side-channel and cross-session
//! concurrency; here runs are simply serialized on the one executor thread.

use std::{collections::HashMap, thread};

use agent_lib::client::LlmClient;
use mag_service::{RunId, ServiceError, SessionConfig, SessionId};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

use crate::{EventBus, driver::SessionDriver};

/// A command delivered to the run executor thread.
enum RunLoopCommand {
    /// Drive one user message for a session.
    Run {
        /// Session the message belongs to.
        session_id: SessionId,
        /// Configuration used to lazily build the session's driver.
        config: SessionConfig,
        /// User-visible text payload.
        text: String,
        /// Channel used to report the started run identity or a start failure.
        reply: oneshot::Sender<Result<RunId, ServiceError>>,
    },
}

/// Handle to the background run executor thread.
///
/// Dropping the handle closes the command channel, which stops the executor loop
/// and joins the thread so no run thread is leaked.
pub(crate) struct RunLoop {
    sender: Option<mpsc::UnboundedSender<RunLoopCommand>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl RunLoop {
    /// Spawns the executor thread bound to `client` and `event_bus`.
    pub(crate) fn spawn(client: Arc<dyn LlmClient>, event_bus: EventBus) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let thread = thread::Builder::new()
            .name("mag-run-loop".to_owned())
            .spawn(move || run_executor(client, event_bus, receiver))
            .expect("spawn mag run loop thread");

        Self {
            sender: Some(sender),
            thread: Some(thread),
        }
    }

    /// Drives one message for `session_id` and returns the started run identity.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Backend`] when the executor cannot accept the run
    /// or the session's driver fails to build.
    pub(crate) async fn run(
        &self,
        session_id: SessionId,
        config: SessionConfig,
        text: String,
    ) -> Result<RunId, ServiceError> {
        let (reply, reply_rx) = oneshot::channel();
        let command = RunLoopCommand::Run {
            session_id,
            config,
            text,
            reply,
        };

        self.sender
            .as_ref()
            .expect("run loop sender is live")
            .send(command)
            .map_err(|_| ServiceError::Backend {
                message: "run executor stopped".to_owned(),
            })?;

        reply_rx.await.map_err(|_| ServiceError::Backend {
            message: "run executor dropped the run before replying".to_owned(),
        })?
    }
}

impl Drop for RunLoop {
    fn drop(&mut self) {
        // Dropping the only sender closes the channel so the executor loop ends.
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Body of the executor thread: builds a current-thread runtime and services run
/// commands until the channel closes.
fn run_executor(
    client: Arc<dyn LlmClient>,
    event_bus: EventBus,
    mut receiver: mpsc::UnboundedReceiver<RunLoopCommand>,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build mag run loop runtime");

    runtime.block_on(async move {
        let mut drivers: HashMap<SessionId, SessionDriver> = HashMap::new();

        while let Some(command) = receiver.recv().await {
            match command {
                RunLoopCommand::Run {
                    session_id,
                    config,
                    text,
                    reply,
                } => {
                    let driver = match drivers.entry(session_id) {
                        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            match SessionDriver::new(&config, client.clone()) {
                                Ok(driver) => entry.insert(driver),
                                Err(error) => {
                                    let _ = reply.send(Err(ServiceError::Backend {
                                        message: error.to_string(),
                                    }));
                                    continue;
                                }
                            }
                        }
                    };

                    let run_id = driver
                        .send_message(session_id, text, event_bus.clone())
                        .await;
                    let _ = reply.send(Ok(run_id));
                }
            }
        }
    });
}
