//! Dynamic subagent instance registry (`docs/dyn-agents.md` §5.2, D3/D6).
//!
//! One [`AgentInstanceRegistry`] per session tracks every agent instance
//! spawned through the `agent` tool: ids, lifecycle status, cooperative
//! cancellation and terminal-state notification. Instances are ephemeral —
//! they are never persisted and never enter the restore path
//! (`docs/dyn-agents.md` §5.2).
//!
//! The registry deliberately holds no session/driver state — those layers only
//! ever inject parameters (`TODO.md` dependency-boundary rule). The `agent`
//! spawn tool, its `agent_result` / `agent_cancel` companions (M3-4), the
//! local-instance drive task, and the origin interaction router live in the
//! [`spawn`] submodule (M3-3); the session driver wires the trio into the
//! supervisor's tool surface and cascades session/run cancellation into
//! [`AgentInstanceRegistry::cancel_all`] (M3-5), and drains terminal
//! notifications into the supervisor's pivot channel / next-turn input
//! prefix (M3-6).

pub(crate) mod spawn;

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex, PoisonError},
};

use agent_lib::facade::CancelHandle;
use tokio::sync::Notify;

/// Report/error characters kept in a completion-notification preview
/// (task spec: the first 200 characters).
const NOTIFICATION_PREVIEW_CHARS: usize = 200;

/// One terminal-state notification of an agent instance, queued for the
/// supervisor's completion-notification drain (M3-6): while a run is in
/// flight the driver injects it through the facade's pivot channel with a
/// `PivotSource::Host { label: "agent:<id>" }` attribution; notifications
/// left over when the run ends are prefixed onto the next turn's user input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InstanceNotification {
    /// Id of the instance that reached a terminal state (the pivot-label
    /// attribution, `agent:<id>`).
    pub(crate) id: String,
    /// Human-readable single-line notification text (report/error preview
    /// included, built by [`AgentInstanceRegistry::push_notification`]).
    pub(crate) text: String,
}

/// Lifecycle status of one agent instance (`docs/dyn-agents.md` §5.2:
/// `running → completed / failed / cancelled`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InstanceStatus {
    /// The instance's drive task is still running.
    Running,
    /// The instance finished; the report is the child agent's final
    /// assistant text (the report contract of `docs/dyn-agents.md` §4).
    Completed {
        /// Final report text produced by the instance.
        report: String,
    },
    /// The instance's drive task ended on an error.
    Failed {
        /// Human-readable failure description.
        error: String,
    },
    /// The instance was cancelled (via [`AgentInstanceRegistry::cancel`] /
    /// [`AgentInstanceRegistry::cancel_all`], or its drive task observed the
    /// fired cancel handle and completed it as cancelled).
    Cancelled,
}

impl InstanceStatus {
    /// Returns whether this is a terminal status (anything but `Running`).
    pub(crate) fn is_terminal(&self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// One live or finished agent instance, shared as `Arc<Instance>` between
/// the registry, the spawn-tool handler and the instance drive task (M3-3).
///
/// The first terminal transition wins: a cancel racing a completion never
/// clobbers the completed report and vice versa (M3-R race requirement).
#[derive(Debug)]
pub(crate) struct Instance {
    /// `"<agent_type>-<n>"` id allocated by
    /// [`AgentInstanceRegistry::next_id`].
    pub id: String,
    /// Name of the definition this instance was spawned from.
    pub agent_type: String,
    /// Nesting depth: `1` for a direct child of the session's root supervisor
    /// (which is itself depth `0`), matching the wire
    /// [`Event::AgentInstanceStarted`](mag_service::Event::AgentInstanceStarted)
    /// field and the interaction-origin attribution of the `agent` tool's
    /// origin router (M3-3).
    pub depth: u32,
    status: Mutex<InstanceStatus>,
    cancel: CancelHandle,
    done: Notify,
}

impl Instance {
    /// Creates a running instance with a fresh cancel handle.
    pub(crate) fn new(id: String, agent_type: String, depth: u32) -> Self {
        Self {
            id,
            agent_type,
            depth,
            status: Mutex::new(InstanceStatus::Running),
            cancel: CancelHandle::new(),
            done: Notify::new(),
        }
    }

    /// Returns a snapshot of the current status.
    pub(crate) fn status(&self) -> InstanceStatus {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Returns the cancel handle the drive task passes to
    /// `Agent::run_full_with_cancel` (M3-3).
    pub(crate) fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
    }

    /// Returns the terminal-state notification source awaited by
    /// `agent_result` (M3-4).
    ///
    /// `Notify::notify_waiters` only wakes tasks already waiting, so a
    /// waiter must create and [`enable`](tokio::sync::futures::Notified::enable)
    /// its `Notified` future *before* re-checking [`status`](Self::status),
    /// otherwise a transition landing between the check and the wait is
    /// missed.
    pub(crate) fn done(&self) -> &Notify {
        &self.done
    }

    /// First-terminal-wins transition; wakes all `done` waiters when this
    /// call performed the transition. Returns `false` when the instance was
    /// already terminal.
    fn transition(&self, status: InstanceStatus) -> bool {
        debug_assert!(
            status.is_terminal(),
            "instances only transition to terminal statuses"
        );
        {
            let mut guard = self.status.lock().unwrap_or_else(PoisonError::into_inner);
            if guard.is_terminal() {
                return false;
            }
            *guard = status;
        }
        self.done.notify_waiters();
        true
    }
}

/// Shared table of every agent instance of one session (`docs/dyn-agents.md`
/// §5.2/§5.4).
///
/// Cheap to clone (inner `Arc`): the `agent` tool handler, each instance
/// drive task and the session driver each hold their own clone.
#[derive(Clone, Debug, Default)]
pub(crate) struct AgentInstanceRegistry {
    inner: Arc<RegistryInner>,
}

#[derive(Debug, Default)]
struct RegistryInner {
    /// Per-agent-type counter backing `next_id` (`"<type>-<n>"`).
    counters: Mutex<BTreeMap<String, u64>>,
    /// All instances keyed by id (BTreeMap keeps `list()` deterministic).
    instances: Mutex<BTreeMap<String, Arc<Instance>>>,
    /// Terminal notifications, drained by the driver into the supervisor's
    /// pivot channel / next-turn input prefix (M3-6).
    notifications: Mutex<VecDeque<InstanceNotification>>,
}

impl AgentInstanceRegistry {
    /// Creates an empty registry.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Allocates the next id for `agent_type`: `"<type>-<n>"` with a
    /// per-type counter starting at 1 and increasing monotonically.
    pub(crate) fn next_id(&self, agent_type: &str) -> String {
        let mut counters = self
            .inner
            .counters
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let n = counters.entry(agent_type.to_owned()).or_insert(0);
        *n += 1;
        format!("{agent_type}-{n}")
    }

    /// Registers a freshly created instance and returns it shared.
    pub(crate) fn register(&self, instance: Instance) -> Arc<Instance> {
        let instance = Arc::new(instance);
        self.inner
            .instances
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(instance.id.clone(), Arc::clone(&instance));
        instance
    }

    /// Looks up one instance by id.
    pub(crate) fn get(&self, id: &str) -> Option<Arc<Instance>> {
        self.inner
            .instances
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    /// Returns all registered instances ordered by id.
    pub(crate) fn list(&self) -> Vec<Arc<Instance>> {
        self.inner
            .instances
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    /// Marks `id` with a terminal status: wakes `done` waiters and pushes a
    /// completion notification for M3-6. This is the terminalization path
    /// used by instance drive tasks for all three terminal states (M3-3).
    ///
    /// Returns `false` when the id is unknown or the instance already
    /// reached a terminal status — the first transition wins, so a racing
    /// `complete`/`cancel` never produces a second notification.
    pub(crate) fn complete(&self, id: &str, status: InstanceStatus) -> bool {
        let Some(instance) = self.get(id) else {
            return false;
        };
        if !instance.transition(status.clone()) {
            return false;
        }
        self.push_notification(&instance, &status);
        true
    }

    /// Requests cancellation of one instance: transitions it to `Cancelled`
    /// (first-terminal-wins) and fires its [`CancelHandle`], waking `done`
    /// waiters. The handle fires only when this call won the race to the
    /// terminal state, so an already-completed report is never clobbered
    /// and the handle fires at most once.
    ///
    /// Returns the status after the call, or `None` for an unknown id
    /// (`agent_cancel` then errors with `list()` attached, M3-4).
    pub(crate) fn cancel(&self, id: &str) -> Option<InstanceStatus> {
        let instance = self.get(id)?;
        if instance.transition(InstanceStatus::Cancelled) {
            instance.cancel_handle().cancel();
            self.push_notification(&instance, &InstanceStatus::Cancelled);
        }
        Some(instance.status())
    }

    /// Cancels every still-running instance (session end / supervisor-run
    /// cancel cascade, M3-5). Already-terminal instances are left untouched.
    pub(crate) fn cancel_all(&self) {
        let candidates: Vec<Arc<Instance>> = self
            .list()
            .into_iter()
            .filter(|instance| !instance.status().is_terminal())
            .collect();
        for instance in candidates {
            if instance.transition(InstanceStatus::Cancelled) {
                instance.cancel_handle().cancel();
                self.push_notification(&instance, &InstanceStatus::Cancelled);
            }
        }
    }

    /// Drains all pending completion notifications (oldest first), consumed
    /// by the session driver's pivot drain / next-turn input prefix (M3-6).
    pub(crate) fn drain_notifications(&self) -> Vec<InstanceNotification> {
        self.inner
            .notifications
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect()
    }

    /// Pushes the terminal notification for `instance`.
    fn push_notification(&self, instance: &Instance, status: &InstanceStatus) {
        let text = match status {
            InstanceStatus::Completed { report } => {
                format!(
                    "agent instance {} completed: {}",
                    instance.id,
                    preview(report)
                )
            }
            InstanceStatus::Failed { error } => {
                format!("agent instance {} failed: {}", instance.id, preview(error))
            }
            InstanceStatus::Cancelled => format!("agent instance {} cancelled", instance.id),
            InstanceStatus::Running => return,
        };
        self.inner
            .notifications
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(InstanceNotification {
                id: instance.id.clone(),
                text,
            });
    }
}

/// Renders a single-line, at-most-[`NOTIFICATION_PREVIEW_CHARS`]-char preview
/// of a report/error for the notification queue.
fn preview(text: &str) -> String {
    let one_line = text.replace('\n', " ");
    let mut chars = one_line.chars();
    let mut out: String = chars.by_ref().take(NOTIFICATION_PREVIEW_CHARS).collect();
    if chars.next().is_some() {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn spawn_instance(registry: &AgentInstanceRegistry, agent_type: &str) -> Arc<Instance> {
        let id = registry.next_id(agent_type);
        registry.register(Instance::new(id, agent_type.to_owned(), 1))
    }

    /// Drains notifications and keeps only their texts (most assertions only
    /// care about the rendered line).
    fn drained_texts(registry: &AgentInstanceRegistry) -> Vec<String> {
        registry
            .drain_notifications()
            .into_iter()
            .map(|notification| notification.text)
            .collect()
    }

    #[test]
    fn ids_increment_per_type_independently() {
        let registry = AgentInstanceRegistry::new();

        assert_eq!(registry.next_id("explorer"), "explorer-1");
        assert_eq!(registry.next_id("explorer"), "explorer-2");
        assert_eq!(registry.next_id("general-purpose"), "general-purpose-1");
        assert_eq!(registry.next_id("explorer"), "explorer-3");
    }

    #[test]
    fn register_get_and_list_round_trip() {
        let registry = AgentInstanceRegistry::new();
        let first = spawn_instance(&registry, "explorer");
        let second = spawn_instance(&registry, "general-purpose");

        let found = registry.get("explorer-1").expect("registered instance");
        assert!(Arc::ptr_eq(&found, &first));
        assert_eq!(found.agent_type, "explorer");
        assert_eq!(found.depth, 1);
        assert_eq!(found.status(), InstanceStatus::Running);

        let listed = registry.list();
        let ids: Vec<&str> = listed.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["explorer-1", "general-purpose-1"]);
        assert!(Arc::ptr_eq(&listed[1], &second));
    }

    #[tokio::test]
    async fn complete_transitions_status_wakes_waiters_and_notifies() {
        let registry = AgentInstanceRegistry::new();
        let instance = spawn_instance(&registry, "explorer");

        // Enable the waiter before the transition so the wake-up cannot be
        // missed (the pattern `done()`'s rustdoc mandates for M3-4).
        let notified = instance.done().notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        assert!(registry.complete(
            "explorer-1",
            InstanceStatus::Completed {
                report: "all done".to_owned(),
            },
        ));
        tokio::time::timeout(Duration::from_secs(1), notified.as_mut())
            .await
            .expect("done waiter is woken");

        assert_eq!(
            instance.status(),
            InstanceStatus::Completed {
                report: "all done".to_owned(),
            }
        );
        assert_eq!(
            drained_texts(&registry),
            ["agent instance explorer-1 completed: all done".to_owned()]
        );

        // First terminal transition wins: a second complete is a no-op and
        // pushes no further notification.
        assert!(!registry.complete("explorer-1", InstanceStatus::Cancelled));
        assert_eq!(
            instance.status(),
            InstanceStatus::Completed {
                report: "all done".to_owned(),
            }
        );
        assert!(registry.drain_notifications().is_empty());
    }

    #[test]
    fn complete_notification_previews_and_flattens_long_reports() {
        let registry = AgentInstanceRegistry::new();
        spawn_instance(&registry, "explorer");
        let long_report = format!("line one\n{}", "x".repeat(500));

        assert!(registry.complete(
            "explorer-1",
            InstanceStatus::Completed {
                report: long_report,
            },
        ));

        let [notification] = registry.drain_notifications().try_into().unwrap();
        assert_eq!(notification.id, "explorer-1");
        let expected_preview = format!("line one {}", "x".repeat(200 - "line one ".len()));
        assert_eq!(
            notification.text,
            format!("agent instance explorer-1 completed: {expected_preview}...")
        );
        assert!(
            !notification.text.contains('\n'),
            "single-line notification"
        );
    }

    #[tokio::test]
    async fn cancel_fires_handle_transitions_and_wakes_waiters() {
        let registry = AgentInstanceRegistry::new();
        let instance = spawn_instance(&registry, "explorer");

        let notified = instance.done().notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        assert_eq!(
            registry.cancel("explorer-1"),
            Some(InstanceStatus::Cancelled)
        );
        assert!(instance.cancel_handle().is_cancelled());
        tokio::time::timeout(Duration::from_secs(1), notified.as_mut())
            .await
            .expect("done waiter is woken");
        assert_eq!(
            drained_texts(&registry),
            ["agent instance explorer-1 cancelled".to_owned()]
        );
    }

    #[test]
    fn cancel_after_completion_keeps_the_completed_report() {
        let registry = AgentInstanceRegistry::new();
        let instance = spawn_instance(&registry, "explorer");
        assert!(registry.complete(
            "explorer-1",
            InstanceStatus::Completed {
                report: "findings".to_owned(),
            },
        ));

        assert_eq!(
            registry.cancel("explorer-1"),
            Some(InstanceStatus::Completed {
                report: "findings".to_owned(),
            })
        );
        assert!(
            !instance.cancel_handle().is_cancelled(),
            "losing cancel does not fire the handle"
        );
        assert_eq!(
            drained_texts(&registry),
            ["agent instance explorer-1 completed: findings".to_owned()]
        );
    }

    #[tokio::test]
    async fn cancel_all_cancels_only_running_instances() {
        let registry = AgentInstanceRegistry::new();
        let completed = spawn_instance(&registry, "explorer");
        let running_a = spawn_instance(&registry, "explorer");
        let running_b = spawn_instance(&registry, "general-purpose");
        assert!(registry.complete(
            "explorer-1",
            InstanceStatus::Failed {
                error: "boom".to_owned(),
            },
        ));

        // Waiters must be enabled before the cascade: `notify_waiters` does
        // not store permits for futures created afterwards.
        let notified_a = running_a.done().notified();
        tokio::pin!(notified_a);
        notified_a.as_mut().enable();
        let notified_b = running_b.done().notified();
        tokio::pin!(notified_b);
        notified_b.as_mut().enable();

        registry.cancel_all();

        assert_eq!(
            completed.status(),
            InstanceStatus::Failed {
                error: "boom".to_owned(),
            },
            "terminal instances are untouched"
        );
        assert!(!completed.cancel_handle().is_cancelled());
        for running in [&running_a, &running_b] {
            assert_eq!(running.status(), InstanceStatus::Cancelled);
            assert!(running.cancel_handle().is_cancelled());
        }
        for notified in [notified_a, notified_b] {
            tokio::time::timeout(Duration::from_secs(1), notified)
                .await
                .expect("done waiter is woken");
        }
        assert_eq!(
            drained_texts(&registry),
            [
                "agent instance explorer-1 failed: boom".to_owned(),
                "agent instance explorer-2 cancelled".to_owned(),
                "agent instance general-purpose-1 cancelled".to_owned(),
            ]
        );

        // A second cascade has nothing left to cancel.
        registry.cancel_all();
        assert!(registry.drain_notifications().is_empty());
    }

    #[test]
    fn unknown_ids_are_no_ops() {
        let registry = AgentInstanceRegistry::new();

        assert!(registry.get("nope-1").is_none());
        assert!(!registry.complete(
            "nope-1",
            InstanceStatus::Completed {
                report: String::new(),
            },
        ));
        assert_eq!(registry.cancel("nope-1"), None);
        assert!(registry.drain_notifications().is_empty());
    }

    #[tokio::test]
    async fn concurrent_registration_allocates_unique_ids() {
        let registry = AgentInstanceRegistry::new();
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let registry = registry.clone();
            tasks.push(tokio::spawn(async move {
                let mut ids = Vec::new();
                for _ in 0..10 {
                    let id = registry.next_id("explorer");
                    registry.register(Instance::new(id.clone(), "explorer".to_owned(), 1));
                    ids.push(id);
                    tokio::task::yield_now().await;
                }
                ids
            }));
        }

        let mut all_ids: Vec<String> = Vec::new();
        for task in tasks {
            all_ids.extend(task.await.expect("registration task"));
        }
        all_ids.sort();
        all_ids.dedup();
        assert_eq!(all_ids.len(), 80, "every registered id is unique");
        assert_eq!(registry.list().len(), 80);
        // Counters never handed out a gap or a duplicate.
        assert_eq!(registry.next_id("explorer"), "explorer-81");
    }
}
