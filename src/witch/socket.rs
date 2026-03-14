//! Unix domain socket listener for out-of-process Witch clients.
//!
//! Spawns a listener thread that accepts connections on a Unix socket.
//! Each connection gets a dedicated handler thread that:
//! 1. Reads `WireRequest` frames from the socket
//! 2. Translates them to `HandleCommand`s on the Witch's existing mpsc channel
//! 3. Blocks on the reply channel
//! 4. Writes `WireResponse` frames back to the socket
//!
//! Connection close exits the handler thread. No explicit cleanup needed —
//! mpsc channels drop naturally.

use std::io;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::oneshot;

use crate::meta::wire::{self, WireRequest, WireResponse};

use super::handle::{HandleCommand, WitchHandle};

/// Handle to the socket listener thread for lifecycle management.
pub(super) struct SocketListenerHandle {
    /// Path to the socket file, for cleanup.
    socket_path: PathBuf,
    /// Shutdown flag — set to true to stop accepting connections.
    shutdown: Arc<AtomicBool>,
    /// Join handle for the listener thread.
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SocketListenerHandle {
    /// Signal the listener to stop and wait for it to exit.
    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);

        // Connect to the socket to unblock the accept() call so the
        // listener thread sees the shutdown flag.
        let _ = std::os::unix::net::UnixStream::connect(&self.socket_path);

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }

        // Clean up socket file.
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Resolve the socket path: `$XDG_RUNTIME_DIR/mm.sock`.
pub fn socket_path() -> Option<PathBuf> {
    WitchHandle::default_socket_path()
}

/// Spawn the socket listener thread.
///
/// Binds the socket synchronously on the calling thread so it is ready for
/// `WitchHandle::connect()` immediately on return. The accept loop runs in
/// the spawned thread.
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
    let listener = match UnixListener::bind(&path) {
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

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    let thread = std::thread::Builder::new()
        .name("socket-listener".into())
        .spawn(move || run_listener(listener, cmd_tx, shutdown_clone))
        .expect("Failed to spawn socket listener thread");

    Some(SocketListenerHandle {
        socket_path: path,
        shutdown,
        thread: Some(thread),
    })
}

/// Listener loop: accept connections, spawn handler per connection.
fn run_listener(
    listener: UnixListener,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<HandleCommand>,
    shutdown: Arc<AtomicBool>,
) {
    for stream in listener.incoming() {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        match stream {
            Ok(stream) => {
                let cmd_tx = cmd_tx.clone();
                std::thread::Builder::new()
                    .name("socket-conn".into())
                    .spawn(move || handle_connection(stream, cmd_tx))
                    .expect("Failed to spawn socket connection handler");
            }
            Err(e) => {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                crate::logging::log_general(format!(
                    "[SOCKET] Accept error: {e}"
                ));
            }
        }
    }

    crate::logging::log_general("[SOCKET] Listener shutting down");
}

/// Per-connection handler: blocking request/response loop.
fn handle_connection(
    stream: std::os::unix::net::UnixStream,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<HandleCommand>,
) {
    let mut reader = io::BufReader::new(stream.try_clone().expect("Failed to clone UnixStream"));
    let mut writer = io::BufWriter::new(stream);

    loop {
        // Read next request frame.
        let request: WireRequest = match wire::read_frame(&mut reader) {
            Ok(req) => req,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                // Clean disconnect.
                return;
            }
            Err(e) => {
                crate::logging::log_general(format!(
                    "[SOCKET] Read error: {e}"
                ));
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
                match rx.blocking_recv() {
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
                match rx.blocking_recv() {
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
        if let Err(e) = wire::write_frame(&mut writer, &response) {
            crate::logging::log_general(format!(
                "[SOCKET] Write error: {e}"
            ));
            return;
        }
    }
}
