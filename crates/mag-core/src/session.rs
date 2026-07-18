//! Per-session driver actors and the manager that owns them.
//!
//! `docs/DESIGN.md` §3.1 mandates **one driver actor per session**: each actor
//! exclusively owns its session's facade [`Agent`](agent_lib::facade::Agent)
//! (through a [`SessionDriver`]) and serializes that session's commands, which
//! sidesteps agent-lib's `&mut self` run surface. Control commands
//! ([`SessionCommand::CancelRun`] / [`SessionCommand::RespondInteraction`]) must
//! travel a side-channel that never borrows the agent mutably, so an in-flight
//! run can never starve them.
//!
//! This module implements that model with **one dedicated OS thread per
//! session**, each hosting a `current_thread` runtime plus a
//! [`LocalSet`] (agent-lib's facade run stream is not `Send`, so a run is driven
//! by a `spawn_local` task pinned to the session's thread). Cancellation flows
//! through a lightweight [`CancelToken`]: the actor flips the token and the run
//! task, selecting the token against the facade stream, drops the stream to
//! abandon the in-flight turn (agent-lib keeps committed history intact).

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use agent_lib::{
    client::LlmClient,
    facade::{AgentSnapshot, FacadeError},
};
use mag_service::{
    Event, InteractionResponseWire, RequestId, RunId, ServiceError, SessionConfig, SessionId,
};
use mag_tools::ToolRegistry;
use tokio::{
    runtime::Builder,
    sync::{Notify, mpsc, oneshot},
    task::LocalSet,
};

use crate::{
    EventBus,
    driver::SessionDriver,
    engine::approval::{AskFrontendDecider, IpcApproval},
    persistence::Persistence,
};

/// A cloneable, one-shot cancellation flag shared between a run task and the
/// controllers that can cancel it.
///
/// Cancellation is terminal: once [`cancel`](CancelToken::cancel) is called the
/// token stays cancelled. [`cancelled`](CancelToken::cancelled) resolves as soon
/// as the flag is set, using a [`Notify`] to wake a parked waiter without a
/// busy-loop. Only the flag and notifier are touched, so signalling never
/// borrows the session's agent.
#[derive(Clone, Debug, Default)]
pub(crate) struct CancelToken {
    inner: Arc<CancelState>,
}

#[derive(Debug, Default)]
struct CancelState {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    /// Marks the token cancelled and wakes any parked [`cancelled`](CancelToken::cancelled) waiter.
    pub(crate) fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    /// Returns whether the token has been cancelled.
    fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Resolves once the token is cancelled.
    ///
    /// The waiter is registered before the second flag check so a
    /// [`cancel`](CancelToken::cancel) racing between the check and the await
    /// cannot be missed.
    pub(crate) async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let notified = self.inner.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }
}

/// A command delivered to a single session's actor.
enum SessionCommand {
    /// Start a run for one user message; replies the started run identity or a
    /// backend error when the session's driver failed to build.
    SendMessage {
        /// User-visible message text.
        text: String,
        /// Channel used to report the started run identity or a start failure.
        reply: oneshot::Sender<Result<RunId, ServiceError>>,
    },
    /// Cancel the session's active run, if any (cancel-token side-channel).
    CancelRun,
    /// Resolve a pending interaction request, waking the parked driver.
    RespondInteraction {
        /// Request identity emitted by [`Event::InteractionRequested`].
        request_id: RequestId,
        /// Interface-supplied response to the pending interaction.
        response: InteractionResponseWire,
        /// Channel used to acknowledge delivery or report a failure.
        reply: oneshot::Sender<Result<(), ServiceError>>,
    },
}

/// Lifecycle state of a session actor's driver.
enum DriverState {
    /// The driver is idle and ready to start the next run.
    ///
    /// Boxed because the facade [`Agent`](agent_lib::facade::Agent) it owns is
    /// far larger than the other variants.
    Idle(Box<SessionDriver>),
    /// A run is in flight; the driver was moved into the run task.
    Running,
    /// The driver failed to build; every run start reports this backend error.
    Failed(String),
}

/// One session's actor: owns the driver and serializes the session's commands.
struct SessionActor {
    session_id: SessionId,
    events: EventBus,
    state: DriverState,
    /// Shared approval handler: injected into the driver's agent and used here to
    /// resolve `RespondInteraction` commands (`docs/DESIGN.md` §3.3).
    approval: Arc<IpcApproval>,
    /// Durable store: each committed run writes its snapshot here (`docs/DESIGN.md`
    /// §3.6).
    store: Arc<Persistence>,
    /// Cancel handle for the active run, present only while a run is in flight.
    cancel: Option<CancelToken>,
    /// Commands received while a run was active, replayed once it finishes.
    deferred: VecDeque<SessionCommand>,
    /// Sender the run task uses to hand the driver back on completion.
    run_done_tx: mpsc::UnboundedSender<Box<SessionDriver>>,
    /// Receiver that reclaims the driver once a run task finishes.
    run_done_rx: mpsc::UnboundedReceiver<Box<SessionDriver>>,
}

impl SessionActor {
    /// Builds an actor from a freshly created (or failed) driver.
    fn new(
        session_id: SessionId,
        events: EventBus,
        driver: Result<SessionDriver, FacadeError>,
        approval: Arc<IpcApproval>,
        store: Arc<Persistence>,
    ) -> Self {
        let (run_done_tx, run_done_rx) = mpsc::unbounded_channel();
        let state = match driver {
            Ok(driver) => DriverState::Idle(Box::new(driver)),
            Err(error) => DriverState::Failed(error.to_string()),
        };
        Self {
            session_id,
            events,
            state,
            approval,
            store,
            cancel: None,
            deferred: VecDeque::new(),
            run_done_tx,
            run_done_rx,
        }
    }

    /// Runs the actor loop until its command channel closes.
    ///
    /// The loop interleaves fresh commands with the reclaimed driver from a
    /// finished run. Control commands (`CancelRun`) are handled even while a run
    /// is in flight because the run advances on a separate `spawn_local` task, so
    /// this loop is always free to service the channel.
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<SessionCommand>) {
        loop {
            // Replay a deferred command as soon as the driver is idle again.
            if let Some(command) = self.take_idle_deferred() {
                self.handle(command);
                continue;
            }

            tokio::select! {
                maybe_command = commands.recv() => match maybe_command {
                    Some(command) => self.handle(command),
                    None => break,
                },
                Some(driver) = self.run_done_rx.recv() => {
                    self.state = DriverState::Idle(driver);
                    self.cancel = None;
                }
            }
        }
    }

    /// Pops the next deferred command, but only while the driver is idle.
    fn take_idle_deferred(&mut self) -> Option<SessionCommand> {
        if matches!(self.state, DriverState::Idle(_)) {
            self.deferred.pop_front()
        } else {
            None
        }
    }

    /// Dispatches one command against the current driver state.
    fn handle(&mut self, command: SessionCommand) {
        match command {
            SessionCommand::SendMessage { text, reply } => self.start_run(text, reply),
            SessionCommand::CancelRun => {
                if let Some(cancel) = &self.cancel {
                    cancel.cancel();
                }
            }
            SessionCommand::RespondInteraction {
                request_id,
                response,
                reply,
            } => {
                let _ = reply.send(self.approval.respond(request_id, response));
            }
        }
    }

    /// Starts a run for `text`, deferring or rejecting when a run cannot start.
    fn start_run(&mut self, text: String, reply: oneshot::Sender<Result<RunId, ServiceError>>) {
        match &self.state {
            DriverState::Failed(message) => {
                let _ = reply.send(Err(ServiceError::Backend {
                    message: message.clone(),
                }));
                return;
            }
            DriverState::Running => {
                // A session's runs are serial: defer until the active run ends.
                self.deferred
                    .push_back(SessionCommand::SendMessage { text, reply });
                return;
            }
            DriverState::Idle(_) => {}
        }

        let DriverState::Idle(mut driver) =
            std::mem::replace(&mut self.state, DriverState::Running)
        else {
            unreachable!("driver is idle in this branch");
        };

        let run_id = driver.next_run_id();
        let _ = self.events.emit(Event::RunStarted {
            id: self.session_id,
            run_id,
        });
        // Reply the run identity before the run drives so the caller can cancel
        // an in-flight run rather than block until it finishes.
        let _ = reply.send(Ok(run_id));

        let cancel = CancelToken::default();
        self.cancel = Some(cancel.clone());
        // Arm the approval handler so cancelling this run also unblocks a driver
        // parked on a pending approval (`docs/DESIGN.md` §3.3).
        self.approval.arm_cancel(cancel.clone());
        let events = self.events.clone();
        let store = self.store.clone();
        let run_done = self.run_done_tx.clone();
        let session_id = self.session_id;
        tokio::task::spawn_local(async move {
            driver
                .run_turn(session_id, text, &events, &cancel, &store)
                .await;
            // Hand the driver back so the session can start its next run.
            let _ = run_done.send(driver);
        });
    }
}

/// Body of a session's dedicated thread: a `current_thread` runtime plus a
/// [`LocalSet`] that drives the actor and its (non-`Send`) run tasks.
///
/// When `restore` is `Some`, the session's facade agent is rebuilt from the
/// persisted [`AgentSnapshot`] (re-injecting the client, tools, and approval
/// handler a snapshot deliberately omits); when `None`, a fresh agent is built
/// from `config` (`docs/DESIGN.md` §3.6).
#[allow(clippy::too_many_arguments)]
fn session_thread(
    session_id: SessionId,
    config: SessionConfig,
    client: Arc<dyn LlmClient>,
    tools: Arc<ToolRegistry>,
    event_bus: EventBus,
    store: Arc<Persistence>,
    restore: Option<AgentSnapshot>,
    commands: mpsc::UnboundedReceiver<SessionCommand>,
) {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build mag session runtime");
    let local = LocalSet::new();
    // Shared approval handler bridges the driver's paused interactions to the
    // event bus and back through `RespondInteraction` (`docs/DESIGN.md` §3.3).
    let approval = Arc::new(IpcApproval::new(
        session_id,
        event_bus.clone(),
        Arc::new(AskFrontendDecider),
    ));
    let driver = match restore {
        Some(snapshot) => SessionDriver::restore(client, &tools, approval.clone(), snapshot),
        None => SessionDriver::new(&config, client, &tools, approval.clone()),
    };
    let actor = SessionActor::new(session_id, event_bus, driver, approval, store);
    local.block_on(&runtime, actor.run(commands));
}

/// Handle to one session's actor thread.
struct SessionHandle {
    /// Entry point for the session's actor commands.
    commands: mpsc::UnboundedSender<SessionCommand>,
    /// The actor thread, joined when the session is deleted or the manager drops.
    thread: thread::JoinHandle<()>,
}

/// Owns the per-session driver actors and routes commands to them by id.
///
/// Without an LLM client no actors are spawned; run-related routing then reports
/// a backend error, mirroring the engine's clientless behaviour.
pub(crate) struct SessionManager {
    client: Option<Arc<dyn LlmClient>>,
    tools: Arc<ToolRegistry>,
    event_bus: EventBus,
    store: Arc<Persistence>,
    handles: Mutex<HashMap<SessionId, SessionHandle>>,
}

impl SessionManager {
    /// Creates a manager bound to `event_bus`, spawning actors only when a
    /// `client` is present. Each spawned session assembles its agent with the
    /// shared `tools` registry (`docs/DESIGN.md` §3.2) and persists its committed
    /// snapshots to `store` (`docs/DESIGN.md` §3.6).
    pub(crate) fn new(
        client: Option<Arc<dyn LlmClient>>,
        tools: Arc<ToolRegistry>,
        event_bus: EventBus,
        store: Arc<Persistence>,
    ) -> Self {
        Self {
            client,
            tools,
            event_bus,
            store,
            handles: Mutex::new(HashMap::new()),
        }
    }

    /// Spawns and registers the actor thread for a newly created session.
    ///
    /// A no-op when the manager has no client. The command sender is registered
    /// synchronously, so a later `send_message` never races the actor thread.
    pub(crate) fn create_session(&self, session_id: SessionId, config: SessionConfig) {
        self.spawn_session(session_id, config, None);
    }

    /// Spawns the actor thread for a persisted session, rebuilding its agent from
    /// `snapshot` when one was captured (`docs/DESIGN.md` §3.6).
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Backend`] when the manager has no client and so
    /// cannot host a session actor.
    pub(crate) fn resume_session(
        &self,
        session_id: SessionId,
        config: SessionConfig,
        snapshot: Option<AgentSnapshot>,
    ) -> Result<(), ServiceError> {
        if self.client.is_none() {
            return Err(ServiceError::Backend {
                message: "no LLM client configured".to_owned(),
            });
        }
        self.spawn_session(session_id, config, snapshot);
        Ok(())
    }

    /// Spawns and registers one session actor thread, fresh or restored.
    ///
    /// A no-op when the manager has no client, mirroring the engine's clientless
    /// behaviour.
    fn spawn_session(
        &self,
        session_id: SessionId,
        config: SessionConfig,
        restore: Option<AgentSnapshot>,
    ) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let tools = self.tools.clone();
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        let event_bus = self.event_bus.clone();
        let store = self.store.clone();
        let thread = thread::Builder::new()
            .name(format!("mag-session-{session_id}"))
            .spawn(move || {
                session_thread(
                    session_id,
                    config,
                    client,
                    tools,
                    event_bus,
                    store,
                    restore,
                    commands_rx,
                )
            })
            .expect("spawn mag session thread");
        self.handles.lock().expect("session handles lock").insert(
            session_id,
            SessionHandle {
                commands: commands_tx,
                thread,
            },
        );
    }

    /// Routes a user message to the session actor and awaits the started run id.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Backend`] when no actor exists (no client) or the
    /// actor stopped before replying.
    pub(crate) async fn send_message(
        &self,
        session_id: SessionId,
        text: String,
    ) -> Result<RunId, ServiceError> {
        let sender = self
            .sender(session_id)
            .ok_or_else(|| ServiceError::Backend {
                message: "no LLM client configured".to_owned(),
            })?;
        let (reply, reply_rx) = oneshot::channel();
        sender
            .send(SessionCommand::SendMessage { text, reply })
            .map_err(|_| ServiceError::Backend {
                message: "session actor stopped".to_owned(),
            })?;
        // Drop this cloned sender before parking on the reply so a deferred run
        // never keeps the command channel open and blocks `delete_session`'s join.
        drop(sender);
        reply_rx.await.map_err(|_| ServiceError::Backend {
            message: "session actor dropped the run before replying".to_owned(),
        })?
    }

    /// Routes a cancel to the session actor; a no-op when no actor exists.
    pub(crate) fn cancel(&self, session_id: SessionId) {
        if let Some(sender) = self.sender(session_id) {
            let _ = sender.send(SessionCommand::CancelRun);
        }
    }

    /// Routes an interaction response to the session actor, which resolves the
    /// matching pending approval (`docs/DESIGN.md` §3.3).
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::InteractionNotFound`] when no actor exists for the
    /// session or the request id is unknown, or [`ServiceError::Backend`] when the
    /// actor stopped before replying.
    pub(crate) async fn respond_interaction(
        &self,
        session_id: SessionId,
        request_id: RequestId,
        response: InteractionResponseWire,
    ) -> Result<(), ServiceError> {
        let Some(sender) = self.sender(session_id) else {
            return Err(ServiceError::InteractionNotFound { request_id });
        };
        let (reply, reply_rx) = oneshot::channel();
        sender
            .send(SessionCommand::RespondInteraction {
                request_id,
                response,
                reply,
            })
            .map_err(|_| ServiceError::Backend {
                message: "session actor stopped".to_owned(),
            })?;
        reply_rx.await.map_err(|_| ServiceError::Backend {
            message: "session actor dropped the response before replying".to_owned(),
        })?
    }

    /// Stops a session's actor thread and forgets it.
    pub(crate) fn delete_session(&self, session_id: SessionId) {
        let handle = self
            .handles
            .lock()
            .expect("session handles lock")
            .remove(&session_id);
        if let Some(handle) = handle {
            // Dropping the sender closes the command channel so the actor loop
            // ends; joining reaps the thread (any in-flight run is abandoned).
            drop(handle.commands);
            let _ = handle.thread.join();
        }
    }

    /// Clones the command sender for `session_id`, if an actor exists.
    fn sender(&self, session_id: SessionId) -> Option<mpsc::UnboundedSender<SessionCommand>> {
        self.handles
            .lock()
            .expect("session handles lock")
            .get(&session_id)
            .map(|handle| handle.commands.clone())
    }
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        let handles = std::mem::take(&mut *self.handles.lock().expect("session handles lock"));
        for (_, handle) in handles {
            drop(handle.commands);
            let _ = handle.thread.join();
        }
    }
}
