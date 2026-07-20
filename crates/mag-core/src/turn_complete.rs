//! Turn-complete notification/callback mechanism (`docs/CLI.md` §4.5,
//! decision D2 companion).
//!
//! mag-core exposes an interface-agnostic **turn-complete hook point**: the
//! session driver emits exactly one internal notification every time a run
//! reaches a terminal state — after a successful run's committed snapshot is
//! persisted, or after a failed/cancelled run has been wound down. The
//! notification never crosses the [`MagService`](mag_service::MagService)
//! wire; interface-side observation stays on `ServiceEvent`.
//!
//! The mechanism is generic, not config-specific: configuration apply
//! (`MagService::apply_config`, `docs/CLI.md` §4.4) is its first consumer, and
//! later consumers (desktop notifications, usage accounting, session-title
//! generation) register the same way through
//! [`Engine::add_turn_complete_listener`](crate::Engine::add_turn_complete_listener)
//! at the engine assembly site.
//!
//! # Failure isolation
//!
//! A listener runs synchronously on the session's driver thread. A panicking
//! listener is caught, logged at warn level, and skipped: it can neither
//! disturb the driver state nor starve the listeners registered after it
//! (`docs/CLI.md` §4.5: "listener panic/错误只记日志，绝不影响 driver 状态").

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, PoisonError, RwLock},
};

use mag_service::SessionId;

/// Terminal shape of one completed turn (`docs/CLI.md` §4.5:
/// `committed | cancelled | failed`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnCompletion {
    /// The run reached its committed consistency point: the facade stream
    /// ended on `Done` and the committed snapshot was persisted before the
    /// terminal event went out (`docs/DESIGN.md` §3.6).
    Committed,
    /// The run ended on a failure (its committed history is unchanged from
    /// the prior committed point).
    Failed,
    /// The run's cancel handle fired while the turn was in flight.
    Cancelled,
}

/// One turn-complete notification payload.
///
/// Carries the session identity and how the turn ended. Later consumers can
/// grow the payload (e.g. the session's pinned config revision and the global
/// revision, as sketched in `docs/CLI.md` §4.5) once sessions pin a
/// `ConfigSnapshot` at creation (M3-6 territory).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnSummary {
    session_id: SessionId,
    completion: TurnCompletion,
}

impl TurnSummary {
    /// Creates the summary for one finished turn.
    #[must_use]
    pub(crate) fn new(session_id: SessionId, completion: TurnCompletion) -> Self {
        Self {
            session_id,
            completion,
        }
    }

    /// The session whose turn completed.
    #[must_use]
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// How the turn ended.
    #[must_use]
    pub fn completion(&self) -> TurnCompletion {
        self.completion
    }
}

/// Interface-agnostic observer invoked once per run terminal
/// (`docs/CLI.md` §4.5).
///
/// Implementations must be cheap and non-blocking: the callback runs
/// synchronously on the session's driver thread right after the terminal
/// event is emitted, so expensive work (desktop notification, persistence)
/// should be handed off to a dedicated task. A panic inside the callback is
/// isolated — it is logged and the remaining listeners still run.
///
/// The callback is intentionally synchronous: it fires from the driver right
/// after the run's mutable stream borrow is released and the facade agent is
/// at rest, so no `.await` is needed to observe the committed state. Async
/// consumers should spawn from inside the callback.
pub trait TurnCompleteListener: Send + Sync {
    /// Called exactly once after every run terminal (committed, failed, or
    /// cancelled) on every live session.
    fn on_turn_complete(&self, summary: &TurnSummary);
}

/// Fan-out registry of [`TurnCompleteListener`]s shared by the engine and its
/// session drivers.
///
/// Cheap to clone (an `Arc` around the registry); every clone sees the same
/// listener set, so a listener registered after the engine (and its session
/// actors) already exist still receives later notifications.
#[derive(Clone, Default)]
pub(crate) struct TurnCompleteHub {
    listeners: Arc<RwLock<Vec<Arc<dyn TurnCompleteListener>>>>,
}

impl std::fmt::Debug for TurnCompleteHub {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TurnCompleteHub")
            .field(
                "listeners",
                &self
                    .listeners
                    .read()
                    .map(|listeners| listeners.len())
                    .unwrap_or(0),
            )
            .finish()
    }
}

impl TurnCompleteHub {
    /// Registers `listener` for future turn-complete notifications.
    pub(crate) fn add_listener(&self, listener: Arc<dyn TurnCompleteListener>) {
        self.listeners
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push(listener);
    }

    /// Delivers `summary` to every registered listener in registration order.
    ///
    /// Each listener is invoked under [`catch_unwind`]: a panic is logged at
    /// warn level and the remaining listeners are still called, so a broken
    /// listener can never disturb the driver or its peers. The listener set
    /// is snapshotted before delivery, so a listener registering (or a
    /// poisoned registry lock recovering) mid-delivery never deadlocks the
    /// driver.
    pub(crate) fn notify(&self, summary: &TurnSummary) {
        let listeners = self
            .listeners
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for listener in listeners {
            let result = catch_unwind(AssertUnwindSafe(|| listener.on_turn_complete(summary)));
            if result.is_err() {
                tracing::warn!(
                    session_id = %summary.session_id(),
                    completion = ?summary.completion(),
                    "turn-complete listener panicked; continuing with remaining listeners"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mag_service::SessionId;
    use uuid::Uuid;

    use super::{TurnCompleteHub, TurnCompleteListener, TurnCompletion, TurnSummary};

    struct PanickingListener;

    impl TurnCompleteListener for PanickingListener {
        fn on_turn_complete(&self, _summary: &TurnSummary) {
            panic!("listener blew up");
        }
    }

    #[derive(Default)]
    struct RecordingListener(Mutex<Vec<TurnCompletion>>);

    impl TurnCompleteListener for RecordingListener {
        fn on_turn_complete(&self, summary: &TurnSummary) {
            self.0
                .lock()
                .expect("recording lock")
                .push(summary.completion());
        }
    }

    #[test]
    fn a_panicking_listener_does_not_starve_later_listeners() {
        let hub = TurnCompleteHub::default();
        hub.add_listener(Arc::new(PanickingListener));
        let recording = Arc::new(RecordingListener::default());
        hub.add_listener(recording.clone());

        let summary = TurnSummary::new(
            SessionId::new(Uuid::from_u128(1)),
            TurnCompletion::Committed,
        );
        hub.notify(&summary);
        hub.notify(&summary);

        assert_eq!(
            *recording.0.lock().expect("recording lock"),
            vec![TurnCompletion::Committed, TurnCompletion::Committed]
        );
    }
}
