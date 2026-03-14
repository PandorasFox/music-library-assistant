//! Dedicated auth thread: session management and user authentication.
//!
//! The auth thread runs its own single-threaded tokio runtime, owning a
//! read-only DB connection for user table lookups and an in-memory session
//! map shared via `ArcSwap` for lock-free token validation.
//!
//! ## Architecture
//!
//! - `AuthHandle` (client-facing): forward login requests via channel (the auth
//!   thread replies directly through the forwarded reply channel), validate
//!   tokens directly via shared `ArcSwap<SessionMap>` (no round-trip).
//! - `AuthThreadHandle` (Witch-facing): lifecycle management.
//!
//! ## Session Storage
//!
//! Sessions live in `ArcSwap<HashMap<[u8; 32], SessionEntry>>`. The auth thread
//! owns writes (clone-mutate-swap); clients read via `Arc::load()` with zero
//! contention. Every process restart clears all sessions (re-login required).

use std::collections::HashMap;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use arc_swap::ArcSwap;
use tokio::sync::mpsc;

use crate::auth::{self, SessionLifetime, SessionToken};
use crate::db::Database;
use crate::meta::protocol::{AuthResponse, ProtocolError, UnauthenticatedResponse};

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

/// The reply type for unauthenticated protocol requests (forwarded from Witch).
type UnauthReply =
    tokio::sync::oneshot::Sender<Result<UnauthenticatedResponse, ProtocolError>>;

enum AuthRequest {
    /// Login request with the client's original reply channel.
    /// The auth thread executes the login and replies directly.
    Login {
        username: String,
        password: String,
        lifetime: SessionLifetime,
        reply: UnauthReply,
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
/// Login forwards the reply channel to the auth thread (which replies directly).
/// Token validation reads the shared session map — no channel round-trip.
#[derive(Clone)]
pub struct AuthHandle {
    request_tx: mpsc::UnboundedSender<AuthRequest>,
    sessions: Arc<ArcSwap<SessionMap>>,
}

impl AuthHandle {
    /// Forward a login request to the auth thread.
    ///
    /// The auth thread owns the reply channel and responds directly to the
    /// client — the Witch never blocks on the result.
    pub fn forward_login(
        &self,
        username: String,
        password: String,
        lifetime: SessionLifetime,
        reply: UnauthReply,
    ) {
        let _ = self.request_tx.send(AuthRequest::Login {
            username,
            password,
            lifetime,
            reply,
        });
    }

    /// Validate a session token. Lock-free — reads directly from shared memory.
    pub fn validate_token(&self, token: &SessionToken) -> bool {
        let hash = auth::hash_token(token);
        let map = self.sessions.load();
        match map.get(&hash) {
            Some(entry) => entry
                .expires_at
                .is_none_or(|exp| Instant::now() < exp),
            None => false,
        }
    }

}

// ============================================================================
// AuthThreadHandle (Witch-facing)
// ============================================================================

/// Witch-facing auth thread handle for lifecycle management.
pub(crate) struct AuthThreadHandle {
    handle: Option<JoinHandle<()>>,
    request_tx: mpsc::UnboundedSender<AuthRequest>,
}

impl AuthThreadHandle {
    /// Notify the auth thread that the database is now available.
    /// Called by the Witch after first-time setup creates the DB.
    pub(crate) fn notify_db_ready(&self) {
        let _ = self.request_tx.send(AuthRequest::DbReady);
    }
}

impl AuthThreadHandle {
    /// Orderly shutdown: send shutdown signal and join the thread.
    pub(crate) fn shutdown(&mut self) {
        let _ = self.request_tx.send(AuthRequest::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
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

/// Spawn the auth thread with its own single-threaded tokio runtime.
/// Returns client handle + Witch-side handle.
///
/// If `db_path` is Some and exists, the thread opens a read-only connection
/// immediately. If None (AwaitingSetup), the thread starts without a DB and
/// opens one when it receives `DbReady`.
pub(crate) fn spawn(db_path: Option<std::path::PathBuf>) -> (AuthHandle, AuthThreadHandle) {
    let (request_tx, request_rx) = mpsc::unbounded_channel();

    let sessions = Arc::new(ArcSwap::from_pointee(HashMap::new()));
    let sessions_for_thread = Arc::clone(&sessions);

    let tx_clone = request_tx.clone();

    let handle = thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("auth-thread: failed to create tokio runtime");
        rt.block_on(auth_thread_main(request_rx, sessions_for_thread, db_path));
    });

    let client_handle = AuthHandle {
        request_tx: tx_clone,
        sessions,
    };

    let witch_handle = AuthThreadHandle {
        handle: Some(handle),
        request_tx,
    };

    (client_handle, witch_handle)
}

// ============================================================================
// Thread Main Loop
// ============================================================================

async fn auth_thread_main(
    mut rx: mpsc::UnboundedReceiver<AuthRequest>,
    sessions: Arc<ArcSwap<SessionMap>>,
    db_path: Option<std::path::PathBuf>,
) {
    crate::logging::log_general("[AUTH] Thread started");

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

    while let Some(request) = rx.recv().await {
        match request {
            AuthRequest::Login {
                username,
                password,
                lifetime,
                reply,
            } => {
                let auth_response = match &db {
                    Some(db) => match attempt_login(db, &username, &password, lifetime, &sessions) {
                        Ok(token) => AuthResponse::Token(token),
                        Err(msg) => AuthResponse::Failed(msg),
                    },
                    None => AuthResponse::Failed("Database not available".to_string()),
                };
                let _ = reply.send(Ok(UnauthenticatedResponse::Auth(auth_response)));
            }

            AuthRequest::DbReady => {
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

            AuthRequest::Shutdown => {
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
    new_map.retain(|_, e| e.expires_at.is_none_or(|exp| now < exp));

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
            request_tx: mpsc::unbounded_channel().0,
            sessions: Arc::clone(&sessions),
        };
        assert!(handle.validate_token(&token));
    }

    #[test]
    fn test_validate_token_garbage() {
        let sessions = Arc::new(ArcSwap::from_pointee(HashMap::new()));
        let handle = AuthHandle {
            request_tx: mpsc::unbounded_channel().0,
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
            request_tx: mpsc::unbounded_channel().0,
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
            request_tx: mpsc::unbounded_channel().0,
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
                    request_tx: mpsc::unbounded_channel().0,
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
