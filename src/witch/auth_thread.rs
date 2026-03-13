//! Dedicated auth thread: session management and user authentication.
//!
//! The auth thread owns a read-only DB connection for user table lookups and
//! an in-memory session map shared via `ArcSwap` for lock-free token validation.
//!
//! ## Architecture
//!
//! - `AuthHandle` (client-facing): send login requests via channel, validate
//!   tokens directly via shared `ArcSwap<SessionMap>` (no round-trip).
//! - `AuthThreadHandle` (Witch-facing): lifecycle management via `ManagedThread`.
//!
//! ## Session Storage
//!
//! Sessions live in `ArcSwap<HashMap<[u8; 32], SessionEntry>>`. The auth thread
//! owns writes (clone-mutate-swap); clients read via `Arc::load()` with zero
//! contention. Every process restart clears all sessions (re-login required).

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use arc_swap::ArcSwap;

use crate::auth::{self, SessionLifetime, SessionToken};
use crate::db::Database;

use super::types::ManagedThread;

// ============================================================================
// Session Storage
// ============================================================================

type SessionMap = HashMap<[u8; 32], SessionEntry>;

#[derive(Debug, Clone)]
struct SessionEntry {
    _user_id: i64,
    _created_at: Instant,
    expires_at: Option<Instant>,
}

// ============================================================================
// Auth Request Protocol
// ============================================================================

enum AuthRequest {
    Login {
        username: String,
        password: String,
        lifetime: SessionLifetime,
        reply: Sender<Result<SessionToken, String>>,
    },
    /// Notify the auth thread that the DB is now available (after first-time setup).
    DbReady,
    Shutdown,
}

// ============================================================================
// AuthHandle (client-facing)
// ============================================================================

/// Client-facing auth handle. Cloneable, Send + Sync.
///
/// Login uses the channel. Token validation reads the shared session map
/// directly — no channel round-trip, no contention.
#[derive(Clone)]
pub struct AuthHandle {
    request_tx: Sender<AuthRequest>,
    sessions: Arc<ArcSwap<SessionMap>>,
}

impl AuthHandle {
    /// Attempt login. Returns a session token on success.
    pub fn login(
        &self,
        username: &str,
        password: &str,
        lifetime: SessionLifetime,
    ) -> Result<SessionToken, String> {
        let (tx, rx) = mpsc::channel();
        let _ = self.request_tx.send(AuthRequest::Login {
            username: username.to_string(),
            password: password.to_string(),
            lifetime,
            reply: tx,
        });
        rx.recv().unwrap_or(Err("Auth thread disconnected".to_string()))
    }

    /// Validate a session token. Lock-free — reads directly from shared memory.
    pub fn validate_token(&self, token: &SessionToken) -> bool {
        let hash = auth::hash_token(token);
        let map = self.sessions.load();
        match map.get(&hash) {
            Some(entry) => entry
                .expires_at
                .map_or(true, |exp| Instant::now() < exp),
            None => false,
        }
    }

}

// ============================================================================
// AuthThreadHandle (Witch-facing)
// ============================================================================

/// Witch-facing auth thread handle for lifecycle management.
pub(crate) struct AuthThreadHandle {
    join_handle: Option<JoinHandle<()>>,
    request_tx: Sender<AuthRequest>,
}

impl AuthThreadHandle {
    /// Notify the auth thread that the database is now available.
    /// Called by the Witch after first-time setup creates the DB.
    pub(crate) fn notify_db_ready(&self) {
        let _ = self.request_tx.send(AuthRequest::DbReady);
    }
}

impl ManagedThread for AuthThreadHandle {
    fn send_shutdown(&self) {
        let _ = self.request_tx.send(AuthRequest::Shutdown);
    }

    fn take_handle(&mut self) -> Option<JoinHandle<()>> {
        self.join_handle.take()
    }
}

impl Drop for AuthThreadHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ============================================================================
// Spawn
// ============================================================================

/// Spawn the auth thread. Returns client handle + Witch-side handle.
///
/// If `db_path` is Some and exists, the thread opens a read-only connection
/// immediately. If None (AwaitingSetup), the thread starts without a DB and
/// opens one when it receives `DbReady`.
pub(crate) fn spawn(db_path: Option<std::path::PathBuf>) -> (AuthHandle, AuthThreadHandle) {
    let (request_tx, request_rx) = mpsc::channel();

    let sessions = Arc::new(ArcSwap::from_pointee(HashMap::new()));
    let sessions_for_thread = Arc::clone(&sessions);

    let tx_clone = request_tx.clone();

    let join_handle = std::thread::Builder::new()
        .name("auth".into())
        .spawn(move || {
            auth_thread_main(request_rx, sessions_for_thread, db_path);
        })
        .expect("Failed to spawn auth thread");

    let client_handle = AuthHandle {
        request_tx: tx_clone,
        sessions,
    };

    let witch_handle = AuthThreadHandle {
        join_handle: Some(join_handle),
        request_tx,
    };

    (client_handle, witch_handle)
}

// ============================================================================
// Thread Main Loop
// ============================================================================

fn auth_thread_main(
    rx: Receiver<AuthRequest>,
    sessions: Arc<ArcSwap<SessionMap>>,
    db_path: Option<std::path::PathBuf>,
) {
    // Open read-only DB connection if path provided and exists
    let mut db: Option<Database> = db_path.and_then(|p| {
        if p.exists() {
            Database::open_read_only(&p)
                .map_err(|e| {
                    crate::logging::log_error(format!("[AUTH] Failed to open DB: {}", e));
                    e
                })
                .ok()
        } else {
            None
        }
    });

    loop {
        match rx.recv() {
            Ok(AuthRequest::Login {
                username,
                password,
                lifetime,
                reply,
            }) => {
                let result = match &db {
                    Some(db) => attempt_login(db, &username, &password, lifetime, &sessions),
                    None => Err("Database not available".to_string()),
                };
                let _ = reply.send(result);
            }

            Ok(AuthRequest::DbReady) => {
                // First-time setup completed — open DB connection
                if db.is_none() {
                    if let Ok(path) = crate::config::get_db_path() {
                        if path.exists() {
                            match Database::open_read_only(&path) {
                                Ok(new_db) => {
                                    crate::logging::log_general(
                                        "[AUTH] DB connection opened after setup",
                                    );
                                    db = Some(new_db);
                                }
                                Err(e) => {
                                    crate::logging::log_error(format!(
                                        "[AUTH] Failed to open DB after setup: {}",
                                        e
                                    ));
                                }
                            }
                        }
                    }
                }
            }

            Ok(AuthRequest::Shutdown) | Err(_) => {
                crate::logging::log_general("[AUTH] Shutting down");
                break;
            }
        }
    }
}

/// Attempt a login against the DB. On success, insert session and return token.
fn attempt_login(
    db: &Database,
    username: &str,
    password: &str,
    lifetime: SessionLifetime,
    sessions: &Arc<ArcSwap<SessionMap>>,
) -> Result<SessionToken, String> {
    let user = db
        .get_user_by_username(username)
        .map_err(|e| format!("Database error: {}", e))?
        .ok_or_else(|| "Invalid username or password".to_string())?;

    let valid = auth::verify_password(password, &user.password_hash)
        .map_err(|e| format!("Password verification error: {}", e))?;

    if !valid {
        return Err("Invalid username or password".to_string());
    }

    // Generate token and insert session
    let token = auth::generate_session_token();
    let token_hash = auth::hash_token(&token);

    let expires_at = match lifetime {
        SessionLifetime::CloseOnExit => None,
    };

    let entry = SessionEntry {
        _user_id: user.id,
        _created_at: Instant::now(),
        expires_at,
    };

    // Clone-mutate-swap
    let current = sessions.load();
    let mut new_map = (**current).clone();

    // Lazily evict expired sessions while we're here
    let now = Instant::now();
    new_map.retain(|_, e| e.expires_at.map_or(true, |exp| now < exp));

    new_map.insert(token_hash, entry);
    sessions.store(Arc::new(new_map));

    Ok(token)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth;
    use mm_utils::t;
    use std::time::Duration;

    #[test]
    fn test_validate_token_after_login() {
        let sessions = Arc::new(ArcSwap::from_pointee(HashMap::new()));
        let db = Database::open_in_memory();

        // Create a user
        let hash = t!(auth::hash_password("secret"));
        t!(db.create_user("alice", &hash));

        // Login
        let token = t!(attempt_login(
            &db,
            "alice",
            "secret",
            SessionLifetime::CloseOnExit,
            &sessions,
        ));

        // Validate via the same sessions arc
        let handle = AuthHandle {
            request_tx: mpsc::channel().0,
            sessions: Arc::clone(&sessions),
        };
        assert!(handle.validate_token(&token));
    }

    #[test]
    fn test_validate_token_garbage() {
        let sessions = Arc::new(ArcSwap::from_pointee(HashMap::new()));
        let handle = AuthHandle {
            request_tx: mpsc::channel().0,
            sessions,
        };
        let garbage = SessionToken::from_bytes(vec![0xDE; 32]);
        assert!(!handle.validate_token(&garbage));
    }

    #[test]
    fn test_login_wrong_password() {
        let sessions = Arc::new(ArcSwap::from_pointee(HashMap::new()));
        let db = Database::open_in_memory();

        let hash = t!(auth::hash_password("correct"));
        t!(db.create_user("alice", &hash));

        let result = attempt_login(
            &db,
            "alice",
            "wrong",
            SessionLifetime::CloseOnExit,
            &sessions,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid username or password"));
    }

    #[test]
    fn test_login_nonexistent_user() {
        let sessions = Arc::new(ArcSwap::from_pointee(HashMap::new()));
        let db = Database::open_in_memory();

        let result = attempt_login(
            &db,
            "nobody",
            "password",
            SessionLifetime::CloseOnExit,
            &sessions,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_expired_token() {
        let sessions = Arc::new(ArcSwap::from_pointee(HashMap::new()));
        let db = Database::open_in_memory();

        let hash = t!(auth::hash_password("secret"));
        t!(db.create_user("alice", &hash));

        // Login with already-expired duration
        let token = auth::generate_session_token();
        let token_hash = auth::hash_token(&token);

        // Manually insert an expired session
        let entry = SessionEntry {
            _user_id: 1,
            _created_at: Instant::now(),
            expires_at: Some(Instant::now() - Duration::from_secs(1)),
        };
        let mut map = HashMap::new();
        map.insert(token_hash, entry);
        sessions.store(Arc::new(map));

        let handle = AuthHandle {
            request_tx: mpsc::channel().0,
            sessions: Arc::clone(&sessions),
        };
        assert!(!handle.validate_token(&token));
    }

    #[test]
    fn test_logout_invalidates_token() {
        let sessions = Arc::new(ArcSwap::from_pointee(HashMap::new()));
        let db = Database::open_in_memory();

        let hash = t!(auth::hash_password("secret"));
        t!(db.create_user("alice", &hash));

        let token = t!(attempt_login(
            &db,
            "alice",
            "secret",
            SessionLifetime::CloseOnExit,
            &sessions,
        ));

        let handle = AuthHandle {
            request_tx: mpsc::channel().0,
            sessions: Arc::clone(&sessions),
        };
        assert!(handle.validate_token(&token));

        // Simulate logout (direct map manipulation since we don't have the thread)
        let token_hash = auth::hash_token(&token);
        let current = sessions.load();
        let mut new_map = (**current).clone();
        new_map.remove(&token_hash);
        sessions.store(Arc::new(new_map));

        assert!(!handle.validate_token(&token));
    }

    #[test]
    fn test_concurrent_validation() {
        let sessions = Arc::new(ArcSwap::from_pointee(HashMap::new()));
        let db = Database::open_in_memory();

        let hash = t!(auth::hash_password("secret"));
        t!(db.create_user("alice", &hash));

        let token = t!(attempt_login(
            &db,
            "alice",
            "secret",
            SessionLifetime::CloseOnExit,
            &sessions,
        ));

        // Spawn multiple threads that all validate the same token
        let mut handles = Vec::new();
        for _ in 0..8 {
            let sessions_clone = Arc::clone(&sessions);
            let token_bytes = token.as_bytes().to_vec();
            handles.push(std::thread::spawn(move || {
                let handle = AuthHandle {
                    request_tx: mpsc::channel().0,
                    sessions: sessions_clone,
                };
                let t = SessionToken::from_bytes(token_bytes);
                for _ in 0..100 {
                    assert!(handle.validate_token(&t));
                }
            }));
        }
        for h in handles {
            t!(h.join());
        }
    }

    #[test]
    fn test_status_no_users() {
        let db = Database::open_in_memory();
        let count = t!(db.user_count());
        assert_eq!(count, 0);
        // Without users → NeedsSetup
    }

    #[test]
    fn test_status_with_users() {
        let db = Database::open_in_memory();
        let hash = t!(auth::hash_password("pass"));
        t!(db.create_user("admin", &hash));
        let count = t!(db.user_count());
        assert_eq!(count, 1);
        // With users → NeedsAuth
    }
}
