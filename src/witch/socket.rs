//! Unix domain socket listener for out-of-process Witch clients.
//!
//! Spawns a tokio task that accepts connections on a Unix socket.
//! Each connection gets a dedicated async task that:
//! 1. Reads `WireRequest` frames from the socket (async)
//! 2. Translates them to `HandleCommand`s on the Witch's mpsc channel
//! 3. Awaits the oneshot reply
//! 4. Writes `WireResponse` frames back to the socket (async)
//!
//! Connection close exits the handler task. No explicit cleanup needed —
//! channels drop naturally.

use std::io;
use std::path::PathBuf;

use tokio::sync::oneshot;

use crate::meta::wire::{self, WireRequest, WireResponse};

use super::handle::{HandleCommand, WitchHandle};

/// Handle to the socket listener task for lifecycle management.
pub(super) struct SocketListenerHandle {
    /// Path to the socket file, for cleanup.
    socket_path: PathBuf,
    /// Tokio task handle for the accept loop.
    task: Option<tokio::task::JoinHandle<()>>,
}

impl SocketListenerHandle {
    /// Abort the listener task and clean up the socket file.
    pub fn shutdown(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

impl Drop for SocketListenerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Resolve the socket path: `$XDG_RUNTIME_DIR/mm.sock`.
pub fn socket_path() -> Option<PathBuf> {
    WitchHandle::default_socket_path()
}

/// Spawn the socket listener as a tokio task.
///
/// Binds the socket synchronously on the calling thread so it is ready for
/// `WitchHandle::connect()` immediately on return. The accept loop runs as
/// an async tokio task.
///
/// Returns a handle for shutdown coordination, or `None` if XDG_RUNTIME_DIR
/// is not set (no socket in that case — in-process only).
pub(super) fn spawn_listener(
    cmd_tx: tokio::sync::mpsc::UnboundedSender<HandleCommand>,
) -> Option<SocketListenerHandle> {
    let path = socket_path()?;

    // Clean up stale socket from a previous run.
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }

    // Bind synchronously so the socket is ready before we return.
    let std_listener = match std::os::unix::net::UnixListener::bind(&path) {
        Ok(l) => {
            crate::logging::log_general(format!(
                "[SOCKET] Listening on {}",
                path.display()
            ));
            l
        }
        Err(e) => {
            crate::logging::log_general(format!(
                "[SOCKET] Failed to bind {}: {e}",
                path.display()
            ));
            return None;
        }
    };

    // Convert to tokio's async listener.
    std_listener
        .set_nonblocking(true)
        .expect("Failed to set socket nonblocking");
    let listener = tokio::net::UnixListener::from_std(std_listener)
        .expect("Failed to convert UnixListener to tokio");

    let task = tokio::spawn(async move {
        run_listener(listener, cmd_tx).await;
    });

    Some(SocketListenerHandle {
        socket_path: path,
        task: Some(task),
    })
}

/// Async accept loop: spawn a task per connection.
async fn run_listener(
    listener: tokio::net::UnixListener,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<HandleCommand>,
) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let cmd_tx = cmd_tx.clone();
                tokio::spawn(async move {
                    handle_connection(stream, cmd_tx).await;
                });
            }
            Err(e) => {
                crate::logging::log_general(format!("[SOCKET] Accept error: {e}"));
            }
        }
    }
}

/// Per-connection handler: async request/response loop.
async fn handle_connection(
    stream: tokio::net::UnixStream,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<HandleCommand>,
) {
    let (read_half, write_half) = tokio::io::split(stream);
    let mut reader = tokio::io::BufReader::new(read_half);
    let mut writer = tokio::io::BufWriter::new(write_half);

    loop {
        // Read next request frame.
        let request: WireRequest = match wire::read_frame_async(&mut reader).await {
            Ok(req) => req,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                // Clean disconnect.
                return;
            }
            Err(e) => {
                crate::logging::log_general(format!("[SOCKET] Read error: {e}"));
                return;
            }
        };

        // Translate to HandleCommand and shuttle through the Witch's mpsc.
        let response = match request {
            WireRequest::Authenticated { token, body } => {
                let (tx, rx) = oneshot::channel();
                if cmd_tx
                    .send(HandleCommand::Authenticated {
                        token,
                        body,
                        reply: tx,
                    })
                    .is_err()
                {
                    // Witch shut down.
                    return;
                }
                match rx.await {
                    Ok(result) => WireResponse::Authenticated(Box::new(result)),
                    Err(_) => return, // Witch dropped the reply channel.
                }
            }
            WireRequest::Unauthenticated(body) => {
                let (tx, rx) = oneshot::channel();
                if cmd_tx
                    .send(HandleCommand::Unauthenticated { body, reply: tx })
                    .is_err()
                {
                    return;
                }
                match rx.await {
                    Ok(result) => WireResponse::Unauthenticated(result),
                    Err(_) => return,
                }
            }
            WireRequest::NotifyDbReady => {
                if cmd_tx.send(HandleCommand::NotifyDbReady).is_err() {
                    return;
                }
                WireResponse::Ack
            }
        };

        // Write response frame.
        if let Err(e) = wire::write_frame_async(&mut writer, &response).await {
            crate::logging::log_general(format!("[SOCKET] Write error: {e}"));
            return;
        }
    }
}
