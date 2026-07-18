//! SQLite persistence for sessions and committed agent snapshots.
//!
//! `docs/DESIGN.md` §3.6 mandates a durable store so a session survives a
//! process restart: the session's configuration is persisted on creation, and an
//! [`AgentSnapshot`] is written after every **committed** run (the only point at
//! which agent-lib's [`AgentState`](agent_lib::agent) can be serialized). A later
//! [`resume`](crate::Engine::resume_session) reads the latest snapshot and
//! rebuilds the session's agent from it.
//!
//! The store keeps only data: the session `config` (JSON) and the data-only
//! [`AgentSnapshot`] JSON blob. It never holds the LLM client, provider
//! credentials, tool closures, or the approval handler — those runtime handles
//! are re-injected on restore (`docs/DESIGN.md` §3.5/§3.6, `PLAN.md` R-D). Storing
//! the snapshot as a JSON blob (with a `schema_meta` version column) keeps the
//! relational schema to a handful of stable columns so it can evolve cheaply.
//!
//! [`rusqlite::Connection`] is `Send` but not `Sync`, so a single connection is
//! guarded by a [`Mutex`] and shared as `Arc<Persistence>` across the engine
//! thread and every per-session actor thread. Each stored operation is a short
//! local write, so lock contention is negligible.

use std::{
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use agent_lib::facade::AgentSnapshot;
use mag_service::{SessionConfig, SessionId, SessionInfo};
use rusqlite::{Connection, OptionalExtension};

/// Current on-disk schema version, stored in `schema_meta` so a future migration
/// can detect and upgrade an older database (`PLAN.md` R-D).
const SCHEMA_VERSION: i64 = 1;

/// An error raised by the SQLite persistence layer.
#[derive(Debug)]
pub enum PersistenceError {
    /// A SQLite operation failed.
    Sqlite(rusqlite::Error),
    /// A stored value could not be (de)serialized as JSON.
    Serde(serde_json::Error),
    /// A stored session identifier could not be parsed back into a [`SessionId`].
    InvalidSessionId(String),
}

impl std::fmt::Display for PersistenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "sqlite error: {error}"),
            Self::Serde(error) => write!(formatter, "persistence serialization error: {error}"),
            Self::InvalidSessionId(value) => {
                write!(formatter, "invalid stored session id `{value}`")
            }
        }
    }
}

impl std::error::Error for PersistenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Serde(error) => Some(error),
            Self::InvalidSessionId(_) => None,
        }
    }
}

impl From<rusqlite::Error> for PersistenceError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<serde_json::Error> for PersistenceError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serde(error)
    }
}

/// SQLite-backed durable store for session configs and committed snapshots.
///
/// A single [`Connection`] is guarded by a [`Mutex`] so the store is `Send +
/// Sync` and can be shared as `Arc<Persistence>` between the engine and its
/// per-session actor threads.
pub(crate) struct Persistence {
    connection: Mutex<Connection>,
}

impl std::fmt::Debug for Persistence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Persistence")
            .finish_non_exhaustive()
    }
}

impl Persistence {
    /// Opens (or creates) a file-backed store at `path`, applying the schema.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError::Sqlite`] when the database cannot be opened or
    /// the schema cannot be applied.
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self, PersistenceError> {
        let connection = Connection::open(path)?;
        Self::from_connection(connection)
    }

    /// Opens a private in-memory store, applying the schema.
    ///
    /// Used by the default (non-persistent) engine constructors so every code
    /// path speaks to the same store surface; the data lives only as long as the
    /// engine.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError::Sqlite`] when the database cannot be opened or
    /// the schema cannot be applied.
    pub(crate) fn in_memory() -> Result<Self, PersistenceError> {
        let connection = Connection::open_in_memory()?;
        Self::from_connection(connection)
    }

    /// Applies the schema to a freshly opened connection.
    fn from_connection(connection: Connection) -> Result<Self, PersistenceError> {
        // Enforce the snapshot -> session foreign key so a deleted session's
        // snapshot is removed with it.
        connection.execute("PRAGMA foreign_keys = ON", [])?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_meta (
                 version INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS sessions (
                 id          TEXT    PRIMARY KEY,
                 config_json TEXT    NOT NULL,
                 created_at  INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS snapshots (
                 session_id           TEXT    PRIMARY KEY,
                 agent_snapshot_json  TEXT    NOT NULL,
                 committed_at         INTEGER NOT NULL,
                 FOREIGN KEY (session_id) REFERENCES sessions (id) ON DELETE CASCADE
             );",
        )?;
        // Record the schema version once.
        if connection
            .query_row("SELECT version FROM schema_meta LIMIT 1", [], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?
            .is_none()
        {
            connection.execute(
                "INSERT INTO schema_meta (version) VALUES (?1)",
                [SCHEMA_VERSION],
            )?;
        }
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Persists a session's configuration, replacing any existing row.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] when the config cannot be serialized or the
    /// row cannot be written.
    pub(crate) fn save_session(
        &self,
        id: SessionId,
        config: &SessionConfig,
    ) -> Result<(), PersistenceError> {
        let config_json = serde_json::to_string(config)?;
        let connection = self.connection.lock().expect("persistence lock");
        connection.execute(
            "INSERT INTO sessions (id, config_json, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (id) DO UPDATE SET config_json = excluded.config_json",
            rusqlite::params![session_key(id), config_json, now_millis()],
        )?;
        Ok(())
    }

    /// Loads a session's configuration, if the session is stored.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] when the row cannot be read or the stored
    /// config cannot be deserialized.
    pub(crate) fn load_session(
        &self,
        id: SessionId,
    ) -> Result<Option<SessionConfig>, PersistenceError> {
        let connection = self.connection.lock().expect("persistence lock");
        let config_json: Option<String> = connection
            .query_row(
                "SELECT config_json FROM sessions WHERE id = ?1",
                rusqlite::params![session_key(id)],
                |row| row.get(0),
            )
            .optional()?;
        match config_json {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    /// Lists every stored session, ordered by creation time.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] when a row cannot be read or a stored id /
    /// config cannot be parsed.
    pub(crate) fn list_sessions(&self) -> Result<Vec<SessionInfo>, PersistenceError> {
        let connection = self.connection.lock().expect("persistence lock");
        let mut statement =
            connection.prepare("SELECT id, config_json FROM sessions ORDER BY created_at, id")?;
        let rows = statement.query_map([], |row| {
            let id: String = row.get(0)?;
            let config_json: String = row.get(1)?;
            Ok((id, config_json))
        })?;

        let mut sessions = Vec::new();
        for row in rows {
            let (id, config_json) = row?;
            let id = SessionId::parse_str(&id)
                .map_err(|_| PersistenceError::InvalidSessionId(id.clone()))?;
            let config = serde_json::from_str(&config_json)?;
            sessions.push(SessionInfo { id, config });
        }
        Ok(sessions)
    }

    /// Deletes a session and its snapshot; a no-op for an unknown session.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] when the delete cannot be executed.
    pub(crate) fn delete_session(&self, id: SessionId) -> Result<(), PersistenceError> {
        let connection = self.connection.lock().expect("persistence lock");
        // `ON DELETE CASCADE` removes the paired snapshot row.
        connection.execute(
            "DELETE FROM sessions WHERE id = ?1",
            rusqlite::params![session_key(id)],
        )?;
        Ok(())
    }

    /// Writes the latest committed [`AgentSnapshot`] for a session, replacing any
    /// prior snapshot.
    ///
    /// The snapshot is serialized as a JSON blob (`PLAN.md` R-D). It is data-only
    /// by construction, so no credential ever reaches the store.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] when the snapshot cannot be serialized or
    /// written.
    pub(crate) fn save_snapshot(
        &self,
        id: SessionId,
        snapshot: &AgentSnapshot,
    ) -> Result<(), PersistenceError> {
        let snapshot_json = serde_json::to_string(snapshot)?;
        let connection = self.connection.lock().expect("persistence lock");
        connection.execute(
            "INSERT INTO snapshots (session_id, agent_snapshot_json, committed_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (session_id) DO UPDATE SET
                 agent_snapshot_json = excluded.agent_snapshot_json,
                 committed_at = excluded.committed_at",
            rusqlite::params![session_key(id), snapshot_json, now_millis()],
        )?;
        Ok(())
    }

    /// Loads a session's latest committed [`AgentSnapshot`], if one was written.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] when the row cannot be read or the stored
    /// snapshot cannot be deserialized.
    pub(crate) fn load_snapshot(
        &self,
        id: SessionId,
    ) -> Result<Option<AgentSnapshot>, PersistenceError> {
        let connection = self.connection.lock().expect("persistence lock");
        let snapshot_json: Option<String> = connection
            .query_row(
                "SELECT agent_snapshot_json FROM snapshots WHERE session_id = ?1",
                rusqlite::params![session_key(id)],
                |row| row.get(0),
            )
            .optional()?;
        match snapshot_json {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    /// Returns the largest `u128` value across every stored session id, if any.
    ///
    /// mag mints session ids as `Uuid::from_u128(counter)`, so the engine seeds a
    /// restarted process's id counter past this value to keep freshly created
    /// sessions from colliding with restored ones (`docs/DESIGN.md` §3.6).
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError`] when a row cannot be read or a stored id
    /// cannot be parsed.
    pub(crate) fn max_session_id_value(&self) -> Result<Option<u128>, PersistenceError> {
        let connection = self.connection.lock().expect("persistence lock");
        let mut statement = connection.prepare("SELECT id FROM sessions")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;

        let mut max = None;
        for row in rows {
            let id = row?;
            let value = SessionId::parse_str(&id)
                .map_err(|_| PersistenceError::InvalidSessionId(id.clone()))?
                .into_uuid()
                .as_u128();
            max = Some(max.map_or(value, |current: u128| current.max(value)));
        }
        Ok(max)
    }
}

/// Renders a [`SessionId`] as the canonical hyphenated UUID used as its row key.
fn session_key(id: SessionId) -> String {
    id.into_uuid().to_string()
}

/// Milliseconds since the Unix epoch, saturating to zero before it.
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent_lib::{
        client::LlmClient,
        facade::{Agent, AgentSnapshot},
        model::usage::Usage,
    };
    use mag_service::{RoutingMode, SessionConfig, SessionId};
    use uuid::Uuid;

    use super::Persistence;
    use crate::test_support::{FakeLlmClient, text_stream_with_usage};

    fn config(model: &str) -> SessionConfig {
        SessionConfig {
            provider: "fake".to_owned(),
            model: model.to_owned(),
            tool_profile: None,
            cwd: None,
            routing: RoutingMode::ModelRouted,
        }
    }

    fn session_id(value: u128) -> SessionId {
        SessionId::new(Uuid::from_u128(value))
    }

    fn usage(input: u32, output: u32) -> Usage {
        Usage {
            input,
            output,
            total: Some(input + output),
            ..Usage::default()
        }
    }

    /// Builds a committed [`AgentSnapshot`] by running one offline turn through a
    /// real facade [`Agent`], so the snapshot mirrors genuine in-memory state.
    async fn committed_snapshot(reply: &str) -> AgentSnapshot {
        let client: Arc<dyn LlmClient> =
            FakeLlmClient::scripted(vec![text_stream_with_usage(&[reply], usage(3, 1))]);
        let mut agent = Agent::builder()
            .client(client)
            .model("fake-model")
            .max_tokens(64)
            .build()
            .expect("build agent");
        agent.run("hello").await.expect("run turn");
        agent.snapshot().expect("snapshot at committed point")
    }

    #[test]
    fn save_and_load_session_round_trips() {
        let store = Persistence::in_memory().expect("open store");
        let id = session_id(1);
        let config = config("model-a");

        assert_eq!(store.load_session(id).expect("load missing"), None);
        store.save_session(id, &config).expect("save session");

        assert_eq!(store.load_session(id).expect("load session"), Some(config));
    }

    #[test]
    fn list_sessions_reports_stored_sessions_in_creation_order() {
        let store = Persistence::in_memory().expect("open store");
        store
            .save_session(session_id(1), &config("a"))
            .expect("save a");
        store
            .save_session(session_id(2), &config("b"))
            .expect("save b");

        let listed = store.list_sessions().expect("list sessions");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, session_id(1));
        assert_eq!(listed[0].config, config("a"));
        assert_eq!(listed[1].id, session_id(2));
        assert_eq!(listed[1].config, config("b"));
    }

    #[tokio::test]
    async fn save_and_load_snapshot_round_trips_with_in_memory_state() {
        let store = Persistence::in_memory().expect("open store");
        let id = session_id(7);
        store
            .save_session(id, &config("model-a"))
            .expect("save session");

        assert!(store.load_snapshot(id).expect("load missing").is_none());

        let snapshot = committed_snapshot("hello").await;
        store.save_snapshot(id, &snapshot).expect("save snapshot");

        let loaded = store.load_snapshot(id).expect("load snapshot");
        assert_eq!(
            loaded,
            Some(snapshot),
            "the loaded snapshot must equal the in-memory snapshot it was written from",
        );
    }

    #[tokio::test]
    async fn stored_snapshot_json_contains_no_credentials() {
        // The snapshot is data-only by construction: it must never smuggle a
        // provider credential into the store (`docs/DESIGN.md` §3.5/§9.5).
        const SECRET: &str = "sk-super-secret-provider-key";

        let snapshot = committed_snapshot("done").await;
        let json = serde_json::to_string(&snapshot).expect("serialize snapshot");

        assert!(
            !json.contains(SECRET),
            "snapshot JSON leaked a credential value",
        );
        let lowered = json.to_lowercase();
        for forbidden in ["api_key", "apikey", "secret", "credential", "password"] {
            assert!(
                !lowered.contains(forbidden),
                "snapshot JSON contains a credential-like key `{forbidden}`: {json}",
            );
        }
    }

    #[tokio::test]
    async fn delete_session_removes_session_and_snapshot() {
        let store = Persistence::in_memory().expect("open store");
        let id = session_id(3);
        store
            .save_session(id, &config("model-a"))
            .expect("save session");
        let snapshot = committed_snapshot("bye").await;
        store.save_snapshot(id, &snapshot).expect("save snapshot");

        store.delete_session(id).expect("delete session");

        assert_eq!(store.load_session(id).expect("load session"), None);
        assert!(
            store.load_snapshot(id).expect("load snapshot").is_none(),
            "deleting a session must cascade to its snapshot",
        );
    }

    #[test]
    fn max_session_id_value_tracks_the_largest_stored_id() {
        let store = Persistence::in_memory().expect("open store");
        assert_eq!(store.max_session_id_value().expect("empty max"), None);

        store
            .save_session(session_id(1), &config("a"))
            .expect("save 1");
        store
            .save_session(session_id(9), &config("b"))
            .expect("save 9");
        store
            .save_session(session_id(4), &config("c"))
            .expect("save 4");

        assert_eq!(store.max_session_id_value().expect("max"), Some(9));
    }
}
