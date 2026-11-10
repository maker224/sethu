//! Tiny embedded persistence layer.
//!
//! The dataset (users, hosts, sessions) is small enough that a JSON document
//! guarded by an `RwLock` is a better trade-off than a database dependency:
//! the binary stays portable, backups are a single file and the format is
//! auditable. Writes are atomic (write temp file, then rename) and the file is
//! created with mode 0600. Sessions are stored as SHA-256 hashes so a leaked
//! data file cannot be replayed against the server.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::util::{hex, now_unix, random_bytes, sha256_hex};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("storage io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("storage file is corrupt: {0}")]
    Corrupt(String),
    #[error("not found")]
    NotFound,
    #[error("already exists")]
    Conflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostKind {
    /// macOS Screen Sharing / any RFB (VNC) server.
    Macos,
    /// iPhone connected to a Mac running WebDriverAgent.
    Ios,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub password_hash: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Host {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub name: String,
    pub kind: HostKind,
    /// `host:port` for macOS, base URL for WebDriverAgent.
    pub target: String,
    #[serde(default)]
    pub notes: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// SHA-256 of the bearer token.
    pub token_hash: String,
    /// CSRF token, handed to the browser and required on unsafe requests.
    pub csrf: String,
    pub user_id: Uuid,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Db {
    #[serde(default)]
    users: Vec<User>,
    #[serde(default)]
    hosts: Vec<Host>,
    #[serde(default)]
    sessions: Vec<Session>,
}

pub struct Store {
    path: PathBuf,
    inner: RwLock<Db>,
}

impl Store {
    pub fn open(data_dir: &Path) -> Result<Self, StoreError> {
        fs::create_dir_all(data_dir)?;
        let path = data_dir.join("sethu.json");

        let db = if path.exists() {
            let raw = fs::read_to_string(&path)?;
            if raw.trim().is_empty() {
                Db::default()
            } else {
                serde_json::from_str(&raw).map_err(|e| StoreError::Corrupt(e.to_string()))?
            }
        } else {
            Db::default()
        };

        let store = Self {
            path,
            inner: RwLock::new(db),
        };
        store.prune_sessions()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    // ----- users ---------------------------------------------------------

    pub fn create_user(&self, email: &str, password_hash: String) -> Result<User, StoreError> {
        self.write(|db| {
            if db.users.iter().any(|u| u.email == email) {
                return Err(StoreError::Conflict);
            }
            let user = User {
                id: Uuid::new_v4(),
                email: email.to_string(),
                password_hash,
                created_at: now_unix(),
            };
            db.users.push(user.clone());
            Ok(user)
        })
    }

    pub fn user_by_email(&self, email: &str) -> Option<User> {
        self.read(|db| db.users.iter().find(|u| u.email == email).cloned())
    }

    pub fn user_by_id(&self, id: Uuid) -> Option<User> {
        self.read(|db| db.users.iter().find(|u| u.id == id).cloned())
    }

    pub fn user_count(&self) -> usize {
        self.read(|db| db.users.len())
    }

    // ----- sessions ------------------------------------------------------

    /// Returns the raw token (only the hash is persisted).
    pub fn create_session(
        &self,
        user_id: Uuid,
        ttl: Duration,
    ) -> Result<(String, Session), StoreError> {
        let token = hex(&random_bytes::<32>());
        let csrf = hex(&random_bytes::<24>());
        let now = now_unix();
        let session = Session {
            token_hash: sha256_hex(token.as_bytes()),
            csrf,
            user_id,
            created_at: now,
            expires_at: now + ttl.as_secs() as i64,
        };
        let stored = session.clone();
        self.write(|db| {
            db.sessions.retain(|s| s.expires_at > now);
            db.sessions.push(stored);
            Ok(())
        })?;
        Ok((token, session))
    }

    pub fn session_by_token(&self, token: &str) -> Option<Session> {
        if token.is_empty() {
            return None;
        }
        let hash = sha256_hex(token.as_bytes());
        self.read(|db| {
            db.sessions
                .iter()
                .find(|s| s.token_hash == hash && s.expires_at > now_unix())
                .cloned()
        })
    }

    pub fn delete_session(&self, token: &str) -> Result<(), StoreError> {
        let hash = sha256_hex(token.as_bytes());
        self.write(|db| {
            db.sessions.retain(|s| s.token_hash != hash);
            Ok(())
        })
    }

    pub fn prune_sessions(&self) -> Result<(), StoreError> {
        let now = now_unix();
        self.write(|db| {
            db.sessions.retain(|s| s.expires_at > now);
            Ok(())
        })
    }

    // ----- hosts ---------------------------------------------------------

    pub fn list_hosts(&self, owner_id: Uuid) -> Vec<Host> {
        self.read(|db| {
            let mut hosts: Vec<Host> = db
                .hosts
                .iter()
                .filter(|h| h.owner_id == owner_id)
                .cloned()
                .collect();
            hosts.sort_by_key(|h| h.created_at);
            hosts
        })
    }

    pub fn host(&self, id: Uuid, owner_id: Uuid) -> Option<Host> {
        self.read(|db| {
            db.hosts
                .iter()
                .find(|h| h.id == id && h.owner_id == owner_id)
                .cloned()
        })
    }

    pub fn create_host(
        &self,
        owner_id: Uuid,
        name: String,
        kind: HostKind,
        target: String,
        notes: Option<String>,
    ) -> Result<Host, StoreError> {
        self.write(|db| {
            let host = Host {
                id: Uuid::new_v4(),
                owner_id,
                name,
                kind,
                target,
                notes,
                created_at: now_unix(),
            };
            db.hosts.push(host.clone());
            Ok(host)
        })
    }

    pub fn update_host(
        &self,
        id: Uuid,
        owner_id: Uuid,
        name: Option<String>,
        notes: Option<Option<String>>,
    ) -> Result<Host, StoreError> {
        self.write(|db| {
            let host = db
                .hosts
                .iter_mut()
                .find(|h| h.id == id && h.owner_id == owner_id)
                .ok_or(StoreError::NotFound)?;
            if let Some(name) = name {
                host.name = name;
            }
            if let Some(notes) = notes {
                host.notes = notes;
            }
            Ok(host.clone())
        })
    }

    pub fn delete_host(&self, id: Uuid, owner_id: Uuid) -> Result<(), StoreError> {
        self.write(|db| {
            let before = db.hosts.len();
            db.hosts.retain(|h| !(h.id == id && h.owner_id == owner_id));
            if db.hosts.len() == before {
                return Err(StoreError::NotFound);
            }
            Ok(())
        })
    }

    // ----- internals -----------------------------------------------------

    fn read<T>(&self, f: impl FnOnce(&Db) -> T) -> T {
        let guard = self.inner.read().unwrap_or_else(|e| e.into_inner());
        f(&guard)
    }

    /// Mutations run against a clone so a failed disk write never leaves the
    /// in-memory state ahead of what is persisted.
    fn write<T>(&self, f: impl FnOnce(&mut Db) -> Result<T, StoreError>) -> Result<T, StoreError> {
        let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let mut next = guard.clone();
        let value = f(&mut next)?;
        self.persist(&next)?;
        *guard = next;
        Ok(value)
    }

    fn persist(&self, db: &Db) -> Result<(), StoreError> {
        let json = serde_json::to_string_pretty(db)
            .map_err(|e| StoreError::Corrupt(format!("serialize: {e}")))?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
        }
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("sethu-test-{}", Uuid::new_v4()));
        let store = Store::open(&dir).unwrap();
        (store, dir)
    }

    #[test]
    fn users_are_unique_and_persisted() {
        let (store, dir) = temp_store();
        let user = store.create_user("a@example.com", "hash".into()).unwrap();
        assert!(matches!(
            store.create_user("a@example.com", "hash".into()),
            Err(StoreError::Conflict)
        ));

        let reopened = Store::open(&dir).unwrap();
        assert_eq!(reopened.user_by_email("a@example.com").unwrap().id, user.id);
    }

    #[test]
    fn sessions_are_hashed_and_expire() {
        let (store, _dir) = temp_store();
        let user = store.create_user("b@example.com", "hash".into()).unwrap();
        let (token, session) = store
            .create_session(user.id, Duration::from_secs(60))
            .unwrap();
        assert!(!store.path().to_string_lossy().contains(&token));
        assert!(store.session_by_token(&token).is_some());
        assert!(store.session_by_token("nope").is_none());
        store.delete_session(&token).unwrap();
        assert!(store.session_by_token(&token).is_none());
        assert_eq!(session.user_id, user.id);
    }

    #[test]
    fn hosts_are_scoped_to_owner() {
        let (store, _dir) = temp_store();
        let alice = store.create_user("alice@example.com", "h".into()).unwrap();
        let bob = store.create_user("bob@example.com", "h".into()).unwrap();
        let host = store
            .create_host(
                alice.id,
                "Mac".into(),
                HostKind::Macos,
                "10.0.0.2:5900".into(),
                None,
            )
            .unwrap();
        assert!(store.host(host.id, alice.id).is_some());
        assert!(store.host(host.id, bob.id).is_none());
        assert!(store.delete_host(host.id, bob.id).is_err());
        assert_eq!(store.list_hosts(alice.id).len(), 1);
        assert_eq!(store.list_hosts(bob.id).len(), 0);
    }
}
