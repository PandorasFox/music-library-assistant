//! Async RPC bridge between the synchronous UI thread and the Witch.
//!
//! The UI thread holds an [`RpcHandle`] and calls [`RpcHandle::send_blocking`]
//! to issue wire requests. A background thread with a single-threaded tokio
//! runtime owns the [`WitchClient`] and multiplexes requests/responses.

use mm_meta::client::WitchClient;
use mm_meta::wire::{WireRequest, WireResponse};

/// Transport-level error from the RPC bridge.
#[derive(Debug)]
pub enum RpcError {
    /// The RPC thread has exited (socket closed, panic, etc.).
    Disconnected,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::Disconnected => write!(f, "RPC thread disconnected"),
        }
    }
}

impl std::error::Error for RpcError {}

/// Request payload sent from UI thread to RPC thread.
struct RpcRequest {
    wire_request: WireRequest,
    reply: tokio::sync::oneshot::Sender<WireResponse>,
}

/// Synchronous handle held by the UI thread.
///
/// All socket I/O is delegated to the background RPC thread via channels.
/// [`send_blocking`](RpcHandle::send_blocking) blocks the calling thread
/// until the response arrives, preserving the existing synchronous call
/// semantics of the TUI's action handlers.
pub struct RpcHandle {
    tx: tokio::sync::mpsc::UnboundedSender<RpcRequest>,
}

impl RpcHandle {
    /// Send a request and block until the response arrives.
    ///
    /// Uses `oneshot::blocking_recv()` to bridge from the sync UI thread
    /// to the async RPC thread.
    pub fn send_blocking(&self, req: WireRequest) -> Result<WireResponse, RpcError> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(RpcRequest {
                wire_request: req,
                reply: reply_tx,
            })
            .map_err(|_| RpcError::Disconnected)?;
        reply_rx.blocking_recv().map_err(|_| RpcError::Disconnected)
    }
}

/// Spawn the RPC thread and return a handle for the UI thread.
///
/// Takes ownership of the raw std `UnixStream` (which must have completed
/// pre-auth). This is the single thread spawn point in mm-tui.
pub fn start_rpc_thread(socket: std::os::unix::net::UnixStream) -> RpcHandle {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<RpcRequest>();

    std::thread::Builder::new()
        .name("mm-rpc".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build tokio runtime for RPC thread");

            rt.block_on(async move {
                socket.set_nonblocking(true).expect("set_nonblocking");
                let async_stream = tokio::net::UnixStream::from_std(socket)
                    .expect("convert socket to async");

                let client = WitchClient::from_stream(async_stream);

                while let Some(req) = rx.recv().await {
                    let client = client.clone();
                    tokio::spawn(async move {
                        match client.send(req.wire_request).await {
                            Ok(resp) => {
                                let _ = req.reply.send(resp);
                            }
                            Err(_) => {
                                // reply channel drops → caller gets Disconnected
                            }
                        }
                    });
                }
            });
        })
        .expect("failed to spawn RPC thread");

    RpcHandle { tx }
}
