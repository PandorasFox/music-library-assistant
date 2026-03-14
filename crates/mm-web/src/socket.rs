use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::{BufReader, BufWriter};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, Mutex};

use mm_meta::protocol::UnauthenticatedBody;
use mm_meta::wire::{read_frame_async, write_frame_async, WireRequest, WireResponse};

/// Single multiplexed connection to the Witch.
///
/// Spawns background reader and writer tasks. Concurrent callers each get
/// a unique `request_id`; the reader task routes responses back to the
/// correct caller via parked oneshot channels.
#[derive(Clone)]
pub struct WitchConnection {
    /// Send requests through the shared writer task.
    request_tx: mpsc::UnboundedSender<WireRequest>,
    /// Parked reply channels, keyed by request_id.
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<WireResponse>>>>,
    /// Next request ID (atomic counter).
    next_id: Arc<AtomicU64>,
}

impl WitchConnection {
    /// Connect to the Witch and spawn reader/writer background tasks.
    pub async fn connect(socket_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path)
            .await
            .with_context(|| format!("connect to witch at {}", socket_path.display()))?;

        let (read_half, write_half) = tokio::io::split(stream);
        let mut reader = BufReader::new(read_half);
        let mut writer = BufWriter::new(write_half);

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<WireResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (request_tx, mut request_rx) = mpsc::unbounded_channel::<WireRequest>();

        // Writer task: drains request_tx → writes frames to socket
        let writer_handle = tokio::spawn(async move {
            while let Some(req) = request_rx.recv().await {
                if write_frame_async(&mut writer, &req).await.is_err() {
                    return;
                }
            }
        });

        // Reader task: reads response frames → dispatches to parked oneshot
        let pending_clone = Arc::clone(&pending);
        let reader_handle = tokio::spawn(async move {
            loop {
                let resp: WireResponse = match read_frame_async(&mut reader).await {
                    Ok(r) => r,
                    Err(_) => return,
                };
                let id = resp.request_id();
                let mut map = pending_clone.lock().await;
                if let Some(tx) = map.remove(&id) {
                    let _ = tx.send(resp);
                }
            }
        });

        // Drop handles — tasks run until the connection drops.
        drop(writer_handle);
        drop(reader_handle);

        Ok(Self {
            request_tx,
            pending,
            next_id: Arc::new(AtomicU64::new(1)),
        })
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
