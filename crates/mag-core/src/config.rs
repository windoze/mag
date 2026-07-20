//! Runtime configuration service (`docs/CLI.md` §4.3, decisions D2/D4).
//!
//! [`ConfigService`] owns the currently effective configuration as an
//! `Arc<ConfigSnapshot>` behind a lock, plus the path of the write-through
//! config file. Sessions pin a snapshot by cloning the `Arc` (cheap), so a
//! later update never disturbs an already-pinned session (snapshot isolation,
//! decision D2).
//!
//! # Update entries
//!
//! `docs/CLI.md` §4.3 lists three update entries — (a) file watch, (b)
//! explicit reload, (c) programmatic update — that converge on the same
//! pipeline: obtain a DTO → resolve to a DO graph → swap `current`,
//! revision + 1. This implementation provides (b) [`ConfigService::reload`]
//! and (c) [`ConfigService::update`]. Entry (a), the file watcher, is
//! **intentionally degraded to explicit reload only**: the `notify` crate
//! could not be added in the offline build environment (`cargo add notify`
//! succeeded at the manifest level but `cargo fetch --offline` failed — the
//! crate file is not in the local registry cache, and fetching requires
//! network access). When a watcher lands it must debounce (~300 ms) and
//! deduplicate the service's own write-through writes (self-write
//! fingerprint) to avoid a write→watch→reload feedback loop; with explicit
//! reload only, that loop cannot occur.
//!
//! # Change signal
//!
//! Every successful apply broadcasts a [`ConfigChange`] on a
//! `tokio::sync::broadcast` channel (see [`ConfigService::subscribe`]). This
//! is the seam where M3-4 bridges configuration changes to the service
//! contract's `ServiceEvent::ConfigChanged{revision}`: `broadcast` is
//! multi-subscriber, lag-tolerant, and matches the existing [`EventBus`]
//! fan-out pattern, so the engine can forward changes without the service
//! layer reaching into a callback registry. Emitting with no subscribers is
//! a no-op.
//!
//! [`EventBus`]: crate::EventBus

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use mag_config::{ConfigDto, ConfigError, ConfigSnapshot};
use tokio::sync::broadcast;

/// Revision stamped on the snapshot a service is constructed with. Every
/// successful `update`/`reload` increments from there (`docs/CLI.md` §4.3:
/// "monotonically increasing, +1 per successful apply").
const INITIAL_REVISION: u64 = 0;

/// Capacity of the [`ConfigChange`] broadcast channel. Subscribers that fall
/// behind observe `Lagged` and can re-read [`ConfigService::current`]; the
/// latest state is always authoritative, so losing intermediate revisions is
/// harmless.
const CHANGE_CHANNEL_CAPACITY: usize = 16;

/// Notification broadcast after every successful configuration apply
/// (`update` / `reload`); carries the revision and the new snapshot.
///
/// This is the M3-3 signal seam: M3-4 forwards it as
/// `ServiceEvent::ConfigChanged{revision}`; other consumers (e.g. a future
/// file watcher or GUI) may subscribe directly.
#[derive(Clone, Debug)]
pub struct ConfigChange {
    revision: u64,
    snapshot: Arc<ConfigSnapshot>,
}

impl ConfigChange {
    /// Revision of the snapshot that just became current.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The snapshot that just became current (cheap `Arc` clone; identical
    /// to what [`ConfigService::current`] returns after the apply).
    #[must_use]
    pub fn snapshot(&self) -> &Arc<ConfigSnapshot> {
        &self.snapshot
    }
}

/// The runtime configuration service (`docs/CLI.md` §4.3).
///
/// Holds `RwLock<Arc<ConfigSnapshot>>` + the write-through file path. The
/// revision counter lives inside the snapshot itself
/// ([`ConfigSnapshot::revision`]), so it can never disagree with the DO
/// graph it stamps.
///
/// All methods are synchronous: the only blocking work is small-file I/O,
/// and the lock is never held across an `.await` (there are none). Writers
/// (`update` / `reload`) hold the write lock for their whole pipeline —
/// resolve → persist → swap — which serializes them and keeps revision
/// assignment, file contents, and the in-memory snapshot mutually
/// consistent.
pub struct ConfigService {
    path: PathBuf,
    current: RwLock<Arc<ConfigSnapshot>>,
    changes: broadcast::Sender<ConfigChange>,
}

impl ConfigService {
    /// Loads the configuration from `path`, falling back to the built-in
    /// default snapshot when the file does not exist.
    ///
    /// The default snapshot is the resolve of an empty [`ConfigDto`]: no
    /// providers/agents/external agents/tools, no session or approval
    /// overrides (effective defaults — e.g. the `Allow` approval tier —
    /// come from the snapshot's effective accessors, `docs/CLI.md` §4.2).
    /// This keeps a missing config file non-fatal for the verification
    /// prototype (`Engine::from_config` / bin wiring, M3-6).
    ///
    /// A file that exists but is unreadable (other than `NotFound`), corrupt,
    /// or fails resolve is an error — startup must not silently downgrade a
    /// broken explicit configuration (`docs/CLI.md` §4.2: assembly failure is
    /// a diagnosable error, not a silent fallback). The file is never created
    /// or written here; writing happens only through [`ConfigService::update`].
    ///
    /// # Errors
    ///
    /// Returns the [`ConfigError`] from reading/parsing/resolving an existing
    /// but broken file.
    pub fn load_or_default(path: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        let path = path.into();
        let snapshot = match ConfigDto::load(&path) {
            Ok(dto) => Arc::new(ConfigSnapshot::resolve(&dto, INITIAL_REVISION)?),
            Err(ConfigError::Io { source, .. }) if source.kind() == ErrorKind::NotFound => {
                Arc::new(ConfigSnapshot::resolve(
                    &ConfigDto::default(),
                    INITIAL_REVISION,
                )?)
            }
            Err(err) => return Err(err),
        };
        Ok(Self::from_parts(path, snapshot))
    }

    /// Creates a service around an already-resolved snapshot. The snapshot's
    /// own revision is preserved as the service's starting revision.
    #[must_use]
    fn from_parts(path: PathBuf, snapshot: Arc<ConfigSnapshot>) -> Self {
        let (changes, _) = broadcast::channel(CHANGE_CHANNEL_CAPACITY);
        Self {
            path,
            current: RwLock::new(snapshot),
            changes,
        }
    }

    /// The write-through config file path this service is bound to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The currently effective snapshot (cheap `Arc` clone). Cloning pins
    /// the snapshot: later updates produce a new snapshot and leave this one
    /// untouched (snapshot isolation, decision D2).
    #[must_use]
    pub fn current(&self) -> Arc<ConfigSnapshot> {
        read_current(&self.current).clone()
    }

    /// Revision of the currently effective snapshot.
    #[must_use]
    pub fn revision(&self) -> u64 {
        read_current(&self.current).revision()
    }

    /// Subscribes to [`ConfigChange`] notifications emitted by future
    /// successful `update`/`reload` calls. Lagging receivers can re-read
    /// [`ConfigService::current`]; the latest snapshot is authoritative.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<ConfigChange> {
        self.changes.subscribe()
    }

    /// Applies a programmatic configuration change (`docs/CLI.md` §4.3
    /// entry (c), the `update_config` write path).
    ///
    /// Pipeline: DTO→DO resolve (validation included) → **write-through**:
    /// atomically persist the DTO to the config file ([`ConfigDto::save_atomic`])
    /// → swap the in-memory snapshot, revision + 1 → broadcast
    /// [`ConfigChange`].
    ///
    /// Write-through ordering matters: the file is persisted *before* the
    /// swap, so if the write fails the in-memory snapshot stays as it was —
    /// memory and file remain consistent (the error is reported to the
    /// caller). Likewise a resolve failure touches neither file nor snapshot.
    ///
    /// # Errors
    ///
    /// Returns the resolve [`ConfigError`] (validation/cross-reference) or
    /// the persistence [`ConfigError::Io`]/[`ConfigError::Serialize`]; in
    /// both cases the current snapshot is unchanged.
    pub fn update(&self, dto: ConfigDto) -> Result<Arc<ConfigSnapshot>, ConfigError> {
        let mut guard = write_current(&self.current);
        let snapshot = Arc::new(ConfigSnapshot::resolve(&dto, guard.revision() + 1)?);
        // Write-through first: a failed write must not swap the snapshot.
        dto.save_atomic(&self.path)?;
        *guard = snapshot.clone();
        drop(guard);
        self.emit_change(&snapshot);
        Ok(snapshot)
    }

    /// Re-reads the config file and applies it (`docs/CLI.md` §4.3 entry
    /// (b), the `reload_config` path — including a future file watcher's
    /// debounced trigger).
    ///
    /// Pipeline: load DTO from disk → resolve (validation included) → swap,
    /// revision + 1 → broadcast [`ConfigChange`]. A missing, unreadable,
    /// corrupt, or invalid file is an error: the current snapshot is kept
    /// and the service stays usable (no crash, no half-applied state). A
    /// watcher-driven automatic reload should downgrade this error to a
    /// warn-level log and keep running — which falls out naturally here,
    /// since failure never swaps the snapshot.
    ///
    /// # Errors
    ///
    /// Returns the load/parse/resolve [`ConfigError`]; the current snapshot
    /// is unchanged.
    pub fn reload(&self) -> Result<Arc<ConfigSnapshot>, ConfigError> {
        let mut guard = write_current(&self.current);
        let dto = ConfigDto::load(&self.path)?;
        let snapshot = Arc::new(ConfigSnapshot::resolve(&dto, guard.revision() + 1)?);
        *guard = snapshot.clone();
        drop(guard);
        self.emit_change(&snapshot);
        Ok(snapshot)
    }

    /// Broadcasts a change notification; emitting with no subscribers is a
    /// successful no-op (mirrors `EventBus::emit`).
    fn emit_change(&self, snapshot: &Arc<ConfigSnapshot>) {
        let _ = self.changes.send(ConfigChange {
            revision: snapshot.revision(),
            snapshot: snapshot.clone(),
        });
    }
}

impl std::fmt::Debug for ConfigService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigService")
            .field("path", &self.path)
            .field("revision", &self.revision())
            .finish_non_exhaustive()
    }
}

/// Read guard with poison recovery (a panicking writer must not wedge the
/// configuration service; the snapshot inside is immutable, so a poisoned
/// lock still yields a consistent value). Mirrors the `PivotQueue` lock
/// discipline in `session.rs`: never held across `.await`.
fn read_current(lock: &RwLock<Arc<ConfigSnapshot>>) -> RwLockReadGuard<'_, Arc<ConfigSnapshot>> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

/// Write-guard counterpart of [`read_current`].
fn write_current(lock: &RwLock<Arc<ConfigSnapshot>>) -> RwLockWriteGuard<'_, Arc<ConfigSnapshot>> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use tokio::sync::broadcast::error::TryRecvError;

    use super::*;

    /// Unique temp directory per test, removed on drop (same pattern as the
    /// `TempDb` helper in `engine.rs` tests).
    struct TempConfigDir(PathBuf);

    impl TempConfigDir {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos();
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let mut path = std::env::temp_dir();
            path.push(format!("mag-cfg-{}-{nanos}-{unique}", std::process::id()));
            fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }

        fn config_path(&self) -> PathBuf {
            self.0.join("config.toml")
        }
    }

    impl Drop for TempConfigDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const SAMPLE_TOML: &str = r#"
[providers.anthropic]
wire = "anthropic"
base_url = "https://api.anthropic.com"
api_key = { env = "ANTHROPIC_API_KEY" }

[agents.default]
provider = "anthropic"
model = "claude-sonnet-4-5"
tools = ["read_file", "grep"]
"#;

    const EDITED_TOML: &str = r#"
[providers.anthropic]
wire = "anthropic"
base_url = "https://api.anthropic.com"
api_key = { env = "ANTHROPIC_API_KEY" }

[agents.default]
provider = "anthropic"
model = "claude-opus-4-1"
tools = ["read_file", "grep"]

[tools.shell]
approval = "ask"
"#;

    const CORRUPT_TOML: &str = "[providers.broken\nwire = ";

    fn sample_dto() -> ConfigDto {
        ConfigDto::parse_str(SAMPLE_TOML).expect("sample TOML parses")
    }

    #[test]
    fn load_or_default_without_a_file_yields_the_builtin_default_snapshot() {
        let dir = TempConfigDir::new();
        let path = dir.config_path();

        let service = ConfigService::load_or_default(&path).expect("missing file yields default");

        let snapshot = service.current();
        assert_eq!(snapshot.revision(), INITIAL_REVISION);
        assert_eq!(service.revision(), INITIAL_REVISION);
        assert!(snapshot.providers().is_empty());
        assert!(snapshot.agents().is_empty());
        assert!(snapshot.external_agents().is_empty());
        assert_eq!(service.path(), path.as_path());
        assert!(!path.exists(), "loading must not create the file");
    }

    #[test]
    fn load_or_default_reads_an_existing_file() {
        let dir = TempConfigDir::new();
        let path = dir.config_path();
        fs::write(&path, SAMPLE_TOML).expect("write config");

        let service = ConfigService::load_or_default(&path).expect("valid file loads");

        let snapshot = service.current();
        assert_eq!(snapshot.revision(), INITIAL_REVISION);
        let agent = snapshot.agent("default").expect("agent resolved");
        assert_eq!(agent.model(), Some("claude-sonnet-4-5"));
        let provider = agent.provider().expect("provider cross-reference");
        assert!(Arc::ptr_eq(
            provider,
            snapshot.provider("anthropic").expect("provider node")
        ));
    }

    #[test]
    fn load_or_default_rejects_a_corrupt_existing_file() {
        let dir = TempConfigDir::new();
        let path = dir.config_path();
        fs::write(&path, CORRUPT_TOML).expect("write corrupt config");

        let err = ConfigService::load_or_default(&path).expect_err("corrupt file is an error");

        assert!(matches!(err, ConfigError::Parse { .. }), "got: {err}");
    }

    #[test]
    fn update_swaps_the_snapshot_and_persists_the_file() {
        let dir = TempConfigDir::new();
        let path = dir.config_path();
        let service = ConfigService::load_or_default(&path).expect("default service");
        let pinned = service.current();

        let applied = service.update(sample_dto()).expect("update succeeds");

        assert_eq!(applied.revision(), INITIAL_REVISION + 1);
        assert!(Arc::ptr_eq(&service.current(), &applied));
        assert_eq!(service.revision(), INITIAL_REVISION + 1);
        // Snapshot isolation: the previously pinned snapshot is untouched.
        assert_eq!(pinned.revision(), INITIAL_REVISION);
        assert!(pinned.providers().is_empty());
        // Write-through: the file on disk matches the applied DTO.
        let on_disk = ConfigDto::load(&path).expect("file persisted");
        assert_eq!(on_disk, sample_dto());
    }

    #[test]
    fn update_keeps_the_snapshot_and_emits_nothing_when_the_write_fails() {
        let dir = TempConfigDir::new();
        let blocker = dir.0.join("blocker");
        let path = blocker.join("config.toml");
        // Load while the parent does not exist yet (missing file → default);
        // then put a regular file where a parent directory is expected so the
        // write-through `save_atomic` fails while resolve still succeeds.
        let service = ConfigService::load_or_default(&path).expect("missing file yields default");
        let mut rx = service.subscribe();
        let pinned = service.current();
        fs::write(&blocker, b"not a directory").expect("write blocker");

        let err = service
            .update(sample_dto())
            .expect_err("unwritable path fails the update");

        assert!(matches!(err, ConfigError::Io { .. }), "got: {err}");
        // Memory/file consistency first: snapshot unchanged, no signal.
        assert!(Arc::ptr_eq(&service.current(), &pinned));
        assert_eq!(service.revision(), INITIAL_REVISION);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn update_rejects_an_invalid_dto_without_touching_file_or_snapshot() {
        let dir = TempConfigDir::new();
        let path = dir.config_path();
        let service = ConfigService::load_or_default(&path).expect("default service");
        let mut rx = service.subscribe();
        let pinned = service.current();
        let invalid = ConfigDto::parse_str(
            r#"
[agents.default]
provider = "ghost"
model = "claude-sonnet-4-5"
"#,
        )
        .expect("invalid-reference DTO still parses");

        let err = service
            .update(invalid)
            .expect_err("dangling provider reference is rejected");

        match err {
            ConfigError::Validation { path, message } => {
                assert_eq!(path, "agents.default.provider");
                assert!(message.contains("ghost"), "got: {message}");
            }
            other => panic!("expected Validation, got: {other}"),
        }
        assert!(Arc::ptr_eq(&service.current(), &pinned));
        assert!(!path.exists(), "failed update must not write the file");
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn reload_picks_up_external_edits() {
        let dir = TempConfigDir::new();
        let path = dir.config_path();
        fs::write(&path, SAMPLE_TOML).expect("write config");
        let service = ConfigService::load_or_default(&path).expect("load config");
        let pinned = service.current();

        // External hand edit (other process / editor), then explicit reload.
        fs::write(&path, EDITED_TOML).expect("external edit");
        let applied = service.reload().expect("reload succeeds");

        assert_eq!(applied.revision(), INITIAL_REVISION + 1);
        let agent = applied.agent("default").expect("agent resolved");
        assert_eq!(agent.model(), Some("claude-opus-4-1"));
        assert!(applied.tools().contains_key("shell"));
        assert!(Arc::ptr_eq(&service.current(), &applied));
        // The previously pinned snapshot still shows the pre-edit content.
        assert_eq!(
            pinned.agent("default").and_then(|a| a.model()),
            Some("claude-sonnet-4-5")
        );
    }

    #[test]
    fn reload_keeps_the_snapshot_and_emits_nothing_when_the_file_is_corrupt() {
        let dir = TempConfigDir::new();
        let path = dir.config_path();
        fs::write(&path, SAMPLE_TOML).expect("write config");
        let service = ConfigService::load_or_default(&path).expect("load config");
        let mut rx = service.subscribe();
        let pinned = service.current();

        fs::write(&path, CORRUPT_TOML).expect("corrupt the file");
        let err = service.reload().expect_err("corrupt file fails reload");

        assert!(matches!(err, ConfigError::Parse { .. }), "got: {err}");
        // No crash, no swap, no signal: the service keeps serving the
        // last-good snapshot.
        assert!(Arc::ptr_eq(&service.current(), &pinned));
        assert_eq!(service.revision(), INITIAL_REVISION);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
        // And it recovers once the file is fixed.
        fs::write(&path, EDITED_TOML).expect("repair the file");
        let applied = service.reload().expect("reload recovers");
        assert_eq!(applied.revision(), INITIAL_REVISION + 1);
    }

    #[test]
    fn reload_rejects_a_semantically_invalid_file_and_keeps_the_snapshot() {
        let dir = TempConfigDir::new();
        let path = dir.config_path();
        fs::write(&path, SAMPLE_TOML).expect("write config");
        let service = ConfigService::load_or_default(&path).expect("load config");
        let pinned = service.current();

        fs::write(
            &path,
            r#"
[agents.default]
provider = "ghost"
"#,
        )
        .expect("write invalid config");
        let err = service.reload().expect_err("invalid file fails reload");

        assert!(matches!(err, ConfigError::Validation { .. }), "got: {err}");
        assert!(Arc::ptr_eq(&service.current(), &pinned));
    }

    #[test]
    fn subscribers_observe_every_successful_apply_in_order() {
        let dir = TempConfigDir::new();
        let path = dir.config_path();
        let service = ConfigService::load_or_default(&path).expect("default service");
        let mut rx = service.subscribe();

        service.update(sample_dto()).expect("update succeeds");
        let change = rx.try_recv().expect("update emits a change");
        assert_eq!(change.revision(), INITIAL_REVISION + 1);
        assert!(Arc::ptr_eq(change.snapshot(), &service.current()));

        fs::write(&path, EDITED_TOML).expect("external edit");
        service.reload().expect("reload succeeds");
        let change = rx.try_recv().expect("reload emits a change");
        assert_eq!(change.revision(), INITIAL_REVISION + 2);
        assert_eq!(
            change.snapshot().agent("default").and_then(|a| a.model()),
            Some("claude-opus-4-1")
        );

        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn concurrent_readers_observe_a_consistent_swap() {
        let dir = TempConfigDir::new();
        let service =
            Arc::new(ConfigService::load_or_default(dir.config_path()).expect("default service"));
        let reader = {
            let service = Arc::clone(&service);
            std::thread::spawn(move || {
                for _ in 0..1000 {
                    let snapshot = service.current();
                    // Either the old or the new snapshot, never a mix.
                    assert!(snapshot.revision() <= 1);
                }
            })
        };

        service.update(sample_dto()).expect("update succeeds");
        reader.join().expect("reader thread");
        assert_eq!(service.revision(), 1);
    }
}
