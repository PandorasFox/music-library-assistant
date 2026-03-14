use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::net::UnixStream;
use tokio::sync::Mutex;

use mm_meta::protocol::UnauthenticatedBody;
use mm_meta::wire::{read_frame_async, write_frame_async, WireRequest, WireResponse};

struct WitchSocket {
    stream: UnixStream,
}

impl WitchSocket {
    async fn connect(path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(path)
            .await
            .with_context(|| format!("connect to witch at {}", path.display()))?;
        Ok(Self { stream })
    }

    async fn send(&mut self, req: &WireRequest) -> Result<WireResponse> {
        write_frame_async(&mut self.stream, req).await?;
        let resp: WireResponse = read_frame_async(&mut self.stream).await?;
        Ok(resp)
    }
}

#[derive(Clone)]
pub struct WitchPool {
    socket_path: PathBuf,
    connections: Arc<Mutex<Vec<WitchSocket>>>,
}

impl WitchPool {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            connections: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Verify the Witch is reachable by sending a SetupQuery.
    pub async fn verify_connectivity(&self) -> Result<()> {
        let req = WireRequest::Unauthenticated(UnauthenticatedBody::SetupQuery);
        let _resp = self.send(&req).await?;
        Ok(())
    }

    /// Send a request through a pooled connection and return the response.
    pub async fn send(&self, req: &WireRequest) -> Result<WireResponse> {
        let mut socket = self.checkout().await?;
        match socket.send(req).await {
            Ok(resp) => {
                self.checkin(socket).await;
                Ok(resp)
            }
            Err(e) => {
                // Connection is broken — drop it, don't return to pool.
                Err(e)
            }
        }
    }

    async fn checkout(&self) -> Result<WitchSocket> {
        {
            let mut pool = self.connections.lock().await;
            if let Some(socket) = pool.pop() {
                return Ok(socket);
            }
        }
        WitchSocket::connect(&self.socket_path).await
    }

    async fn checkin(&self, socket: WitchSocket) {
        let mut pool = self.connections.lock().await;
        pool.push(socket);
    }
}
