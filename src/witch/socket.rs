//! Unix domain socket listener for out-of-process Witch clients.
//!
//! Spawns a tokio task that accepts connections on a Unix socket.
//! Each connection gets a dedicated async task that runs a multiplexed
//! reader/writer pair:
//!
//! - **Reader task**: reads `WireRequest` frames and spawns a dispatch task
//!   per request, allowing multiple requests to be in-flight concurrently.
//! - **Writer task**: drains completed `WireResponse` frames and pushed
//!   `WitchEvent` broadcasts back to the client.
//!
//! Connection close exits the handler task. No explicit cleanup needed —
//! channels drop naturally.

use std::io;
use std::path::PathBuf;

use crate::meta::wire::{self, WireRequest, WireResponse};
use mm_meta::witch_types::WitchEvent;

use super::handle::HandleCommand;

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
    mm_meta::wire::default_socket_path()
}

/// Spawn the socket listener as a tokio task.
///
/// Binds the socket synchronously on the calling thread so it is ready for
/// client connections immediately on return. The accept loop runs as
/// an async tokio task.
///
/// Returns a handle for shutdown coordination, or `None` if XDG_RUNTIME_DIR
/// is not set (no socket in that case — in-process only).
pub(super) fn spawn_listener(
    cmd_tx: tokio::sync::mpsc::UnboundedSender<HandleCommand>,
    event_tx: tokio::sync::broadcast::Sender<WitchEvent>,
) -> Option<SocketListenerHandle> {
    let path = socket_path()?;

    // Clean up stale socket from a previous run.
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }

    // Bind synchronously so the socket is ready before we return.
    let std_listener = match std::os::unix::net::UnixListener::bind(&path) {
        Ok(l) => {
            // Set socket permissions to 0770 for group access (clients need write to connect)
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o770)) {
                crate::logging::log_general(format!(
                    "[SOCKET] Warning: failed to set permissions on {}: {e}",
                    path.display()
                ));
            }
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
        run_listener(listener, cmd_tx, event_tx).await;
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
    event_tx: tokio::sync::broadcast::Sender<WitchEvent>,
) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let cmd_tx = cmd_tx.clone();
                // NOTE: event subscription starts before authentication.
                // mm-web connects over this socket without per-request auth
                // and relies on status pushes to feed its own web clients.
                // Mild info leak (status snapshots to unauthenticated connections)
                // accepted: socket is local-only ($XDG_RUNTIME_DIR), leaked
                // data is operational status not corpus content.
                let event_rx = event_tx.subscribe();
                tokio::spawn(async move {
                    handle_connection(stream, cmd_tx, event_rx).await;
                });
            }
            Err(e) => {
                crate::logging::log_general(format!("[SOCKET] Accept error: {e}"));
            }
        }
    }
}

/// Per-connection handler: multiplexed reader/writer with spawned dispatch.
async fn handle_connection(
    stream: tokio::net::UnixStream,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<HandleCommand>,
    mut event_rx: tokio::sync::broadcast::Receiver<WitchEvent>,
) {
    let (read_half, write_half) = tokio::io::split(stream);
    let mut reader = tokio::io::BufReader::new(read_half);
    let mut writer = tokio::io::BufWriter::new(write_half);

    // Response channel: dispatch tasks send completed responses here.
    let (resp_tx, mut resp_rx) = tokio::sync::mpsc::unbounded_channel::<WireResponse>();

    tokio::select! {
        // Reader: read requests, spawn dispatch tasks.
        _ = async {
            loop {
                let request: WireRequest = match wire::read_frame_async(&mut reader).await {
                    Ok(req) => req,
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return,
                    Err(e) => {
                        crate::logging::log_general(format!("[SOCKET] Read error: {e}"));
                        return;
                    }
                };

                let cmd_tx = cmd_tx.clone();
                let resp_tx = resp_tx.clone();
                tokio::spawn(async move {
                    let response = dispatch_request(request, cmd_tx).await;
                    let _ = resp_tx.send(response);
                });
            }
        } => {}

        // Writer: interleave completed responses and pushed events.
        _ = async {
            loop {
                tokio::select! {
                    Some(response) = resp_rx.recv() => {
                        if wire::write_frame_async(&mut writer, &response).await.is_err() {
                            return;
                        }
                    }
                    result = event_rx.recv() => {
                        match result {
                            Ok(event) => {
                                let frame = WireResponse::Event(event);
                                if wire::write_frame_async(&mut writer, &frame).await.is_err() {
                                    return;
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                // Client fell behind — stale snapshots skipped, next recv is fresh
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                        }
                    }
                }
            }
        } => {}
    }
}

/// Dispatch a single request: send to Witch, await reply, wrap with request_id.
async fn dispatch_request(
    request: WireRequest,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<HandleCommand>,
) -> WireResponse {
    let request_id = request.request_id();

    match request {
        WireRequest::Authenticated { token, body, .. } => {
            let (tx, rx) = tokio::sync::oneshot::channel();
            if cmd_tx
                .send(HandleCommand::Authenticated {
                    token,
                    body,
                    reply: tx,
                })
                .is_err()
            {
                return WireResponse::Authenticated {
                    request_id,
                    result: Box::new(Err(crate::meta::protocol::ProtocolError::Internal(
                        "server shutting down".to_string(),
                    ))),
                };
            }
            match rx.await {
                Ok(result) => WireResponse::Authenticated {
                    request_id,
                    result: Box::new(result),
                },
                Err(_) => WireResponse::Authenticated {
                    request_id,
                    result: Box::new(Err(crate::meta::protocol::ProtocolError::Internal(
                        "server dropped reply".to_string(),
                    ))),
                },
            }
        }
        WireRequest::Unauthenticated { body, .. } => {
            let (tx, rx) = tokio::sync::oneshot::channel();
            if cmd_tx
                .send(HandleCommand::Unauthenticated { body, reply: tx })
                .is_err()
            {
                return WireResponse::Unauthenticated {
                    request_id,
                    result: Err(crate::meta::protocol::ProtocolError::Internal(
                        "server shutting down".to_string(),
                    )),
                };
            }
            match rx.await {
                Ok(result) => WireResponse::Unauthenticated {
                    request_id,
                    result,
                },
                Err(_) => WireResponse::Unauthenticated {
                    request_id,
                    result: Err(crate::meta::protocol::ProtocolError::Internal(
                        "server dropped reply".to_string(),
                    )),
                },
            }
        }
        WireRequest::NotifyDbReady { .. } => {
            let _ = cmd_tx.send(HandleCommand::NotifyDbReady);
            WireResponse::Ack { request_id }
        }
    }
}
