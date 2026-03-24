//! Multiplexed async client for the Witch socket protocol.
//!
//! Spawns background reader and writer tokio tasks. Concurrent callers
//! each get a unique `request_id`; the reader task routes responses back
//! via parked oneshot channels.
//!
//! Server-pushed events (`WireResponse::Event`) are forwarded to a
//! broadcast channel — callers subscribe via [`WitchClient::subscribe_events`].
//!
//! Used by both mm-web (directly in async context) and mm-tui (via
//! RpcHandle on a background thread with a single-threaded runtime).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::{BufReader, BufWriter};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};

use crate::protocol::UnauthenticatedBody;
use crate::wire::{read_frame_async, write_frame_async, WireRequest, WireResponse};
use crate::witch_types::WitchEvent;

/// Multiplexed async client for the Witch socket protocol.
#[derive(Clone)]
pub struct WitchClient {
    /// Send requests through the shared writer task.
    request_tx: mpsc::UnboundedSender<WireRequest>,
    /// Parked reply channels, keyed by request_id.
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<WireResponse>>>>,
    /// Next request ID (atomic counter).
    next_id: Arc<AtomicU64>,
    /// Broadcast channel for server-pushed events.
    event_tx: broadcast::Sender<WitchEvent>,
}

impl WitchClient {
    /// Connect to the Witch at the given socket path.
    pub async fn connect(socket_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path)
            .await
            .with_context(|| format!("connect to witch at {}", socket_path.display()))?;
        Ok(Self::init(stream))
    }

    /// Wrap an already-connected async UnixStream.
    ///
    /// Used by mm-tui where the socket is opened synchronously for pre-auth,
    /// then converted to async and handed off after login.
    pub fn from_stream(stream: UnixStream) -> Self {
        Self::init(stream)
    }

    /// Shared initialization: split stream, spawn reader/writer tasks.
    fn init(stream: UnixStream) -> Self {
        let (read_half, write_half) = tokio::io::split(stream);
        let mut reader = BufReader::new(read_half);
        let mut writer = BufWriter::new(write_half);

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<WireResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (request_tx, mut request_rx) = mpsc::unbounded_channel::<WireRequest>();
        let (event_tx, _) = broadcast::channel::<WitchEvent>(16);

        // Writer task: drains request_tx → writes frames to socket
        let writer_handle = tokio::spawn(async move {
            while let Some(req) = request_rx.recv().await {
                if write_frame_async(&mut writer, &req).await.is_err() {
                    return;
                }
            }
        });

        // Reader task: routes responses to parked oneshots, events to broadcast
        let pending_clone = Arc::clone(&pending);
        let event_tx_clone = event_tx.clone();
        let reader_handle = tokio::spawn(async move {
            loop {
                let resp: WireResponse = match read_frame_async(&mut reader).await {
                    Ok(r) => r,
                    Err(_) => return,
                };
                match resp {
                    WireResponse::Event(event) => {
                        let _ = event_tx_clone.send(event);
                    }
                    other => {
                        let id = other.request_id();
                        let mut map = pending_clone.lock().await;
                        if let Some(tx) = map.remove(&id) {
                            let _ = tx.send(other);
                        }
                    }
                }
            }
        });

        // Drop handles — tasks run until the connection drops.
        drop(writer_handle);
        drop(reader_handle);

        Self {
            request_tx,
            pending,
            next_id: Arc::new(AtomicU64::new(1)),
            event_tx,
        }
    }

    /// Subscribe to server-pushed events.
    pub fn subscribe_events(&self) -> broadcast::Receiver<WitchEvent> {
        self.event_tx.subscribe()
    }

    /// Verify the Witch is reachable by sending a SetupQuery.
    pub async fn verify_connectivity(&self) -> Result<()> {
        let req = WireRequest::Unauthenticated {
            request_id: 0,
            body: UnauthenticatedBody::SetupQuery,
        };
        let _resp = self.send(req).await?;
        Ok(())
    }

    /// Send a request and await the matching response.
    pub async fn send(&self, mut req: WireRequest) -> Result<WireResponse> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        // Stamp the request_id
        match &mut req {
            WireRequest::Authenticated { request_id, .. } => *request_id = id,
            WireRequest::Unauthenticated { request_id, .. } => *request_id = id,
            WireRequest::NotifyDbReady { request_id } => *request_id = id,
        }

        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        self.request_tx
            .send(req)
            .map_err(|_| anyhow::anyhow!("writer task closed"))?;

        rx.await
            .map_err(|_| anyhow::anyhow!("reader task closed before response arrived"))
    }
}
