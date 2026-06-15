//! Wire transport types and framing for socket-based protocol communication.
//!
//! Provides:
//! - `WireRequest` / `WireResponse` — envelope types mapping 1:1 to `HandleCommand`
//!   minus lifecycle variants (Shutdown) and minus in-process reply channels.
//! - `HandleCommand` — server-internal command type with reply channels.
//! - `default_socket_path()` — XDG-based socket path helper.
//! - Length-prefixed bincode framing in both sync (`std::io`) and async (`tokio::io`) variants.
//!
//! Sync framing is used by the TUI client (blocking socket I/O).
//! Async framing is used by the server-side socket handler (tokio tasks).

use std::io::{self, Read, Write};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

use crate::auth::SessionToken;
use crate::protocol::{
    AuthenticatedBody, AuthenticatedResponse, ProtocolError, UnauthenticatedBody,
    UnauthenticatedResponse,
};
use crate::witch_types::WitchEvent;

// ============================================================================
// Wire Envelope Types
// ============================================================================

/// Client → Server request envelope.
///
/// Maps 1:1 to `HandleCommand` minus `Shutdown` (connection close = shutdown)
/// and minus reply channels (framed response follows on the same stream).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireRequest {
    /// Authenticated protocol request (queries, transactions, commands).
    Authenticated {
        request_id: u64,
        token: SessionToken,
        body: Box<AuthenticatedBody>,
    },
    /// Unauthenticated protocol request (login, setup query).
    Unauthenticated {
        request_id: u64,
        body: UnauthenticatedBody,
    },
    /// Notify server that DB is ready (post-setup lifecycle signal).
    NotifyDbReady {
        request_id: u64,
    },
}

impl WireRequest {
    pub fn request_id(&self) -> u64 {
        match self {
            Self::Authenticated { request_id, .. } => *request_id,
            Self::Unauthenticated { request_id, .. } => *request_id,
            Self::NotifyDbReady { request_id } => *request_id,
        }
    }
}

/// Server → Client response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireResponse {
    /// Response to an authenticated request.
    Authenticated {
        request_id: u64,
        result: Box<Result<AuthenticatedResponse, ProtocolError>>,
    },
    /// Response to an unauthenticated request.
    Unauthenticated {
        request_id: u64,
        result: Result<UnauthenticatedResponse, ProtocolError>,
    },
    /// Acknowledgement for lifecycle signals (NotifyDbReady).
    Ack {
        request_id: u64,
    },
    /// Unsolicited server push — not a response to any client request.
    Event(WitchEvent),
}

impl WireResponse {
    pub fn request_id(&self) -> u64 {
        match self {
            Self::Authenticated { request_id, .. } => *request_id,
            Self::Unauthenticated { request_id, .. } => *request_id,
            Self::Ack { request_id } => *request_id,
            Self::Event(_) => 0,
        }
    }
}

// ============================================================================
// Server-Internal Command Type
// ============================================================================

/// A command sent from connection handlers to the Witch's event loop.
///
/// Three variants: authenticated protocol request, unauthenticated protocol
/// request, and lifecycle (shutdown/notify). The reply channel carries the
/// area-matching response type.
pub enum HandleCommand {
    /// Authenticated protocol request (queries, transactions, commands).
    Authenticated {
        token: SessionToken,
        body: Box<AuthenticatedBody>,
        reply: oneshot::Sender<Result<AuthenticatedResponse, ProtocolError>>,
    },

    /// Unauthenticated protocol request (login, setup).
    Unauthenticated {
        body: UnauthenticatedBody,
        reply: oneshot::Sender<Result<UnauthenticatedResponse, ProtocolError>>,
    },

    /// Notify auth thread that DB is now available (after first-time setup).
    NotifyDbReady,

    /// Shutdown the Witch.
    Shutdown,
}

// ============================================================================
// Socket Path
// ============================================================================

/// Well-known system socket path for the mm service.
pub const SYSTEM_SOCKET_PATH: &str = "/run/mm/mm.sock";

/// Default socket path for creating a socket (server): `$XDG_RUNTIME_DIR/mm.sock`.
///
/// Returns `None` if `XDG_RUNTIME_DIR` is not set.
pub fn default_socket_path() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(|dir| std::path::Path::new(&dir).join("mm.sock"))
}

/// Find an existing socket to connect to (client).
///
/// Checks in order:
/// 1. `$XDG_RUNTIME_DIR/mm.sock` (user's local mm instance)
/// 2. `/run/mm/mm.sock` (system service)
///
/// Returns `None` if no socket exists.
pub fn find_socket_path() -> Option<std::path::PathBuf> {
    // Try user's XDG runtime socket first
    if let Some(path) = default_socket_path() {
        if path.exists() {
            return Some(path);
        }
    }

    // Fall back to system service socket
    let system_path = std::path::PathBuf::from(SYSTEM_SOCKET_PATH);
    if system_path.exists() {
        return Some(system_path);
    }

    None
}

// ============================================================================
// Length-Prefixed Bincode Framing
// ============================================================================

/// Maximum frame size: 64 MiB. Domain query results (e.g. full corpus tag
/// dumps) can be large, but 64 MiB is generous headroom.
pub const MAX_FRAME_SIZE: u32 = 64 * 1024 * 1024;

/// Write a length-prefixed bincode frame.
///
/// Wire format: `[u32 big-endian length] [bincode payload]`
pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let payload = bincode::serialize(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let len = payload.len() as u32;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&payload)?;
    w.flush()
}

/// Read a length-prefixed bincode frame.
///
/// Returns `UnexpectedEof` on clean connection close (zero bytes read).
pub fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<T> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame size {len} exceeds maximum {MAX_FRAME_SIZE}"),
        ));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    bincode::deserialize(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

// ============================================================================
// Async Length-Prefixed Bincode Framing
// ============================================================================

/// Write a length-prefixed bincode frame (async).
///
/// Wire format: `[u32 big-endian length] [bincode payload]`
pub async fn write_frame_async<W: tokio::io::AsyncWrite + Unpin, T: Serialize>(
    w: &mut W,
    msg: &T,
) -> io::Result<()> {
    let payload =
        bincode::serialize(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let len = payload.len() as u32;
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(&payload).await?;
    w.flush().await
}

/// Read a length-prefixed bincode frame (async).
///
/// Returns `UnexpectedEof` on clean connection close (zero bytes read).
pub async fn read_frame_async<R: tokio::io::AsyncRead + Unpin, T: DeserializeOwned>(
    r: &mut R,
) -> io::Result<T> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame size {len} exceeds maximum {MAX_FRAME_SIZE}"),
        ));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).await?;
    bincode::deserialize(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip_wire_request() {
        let req = WireRequest::Unauthenticated {
            request_id: 42,
            body: UnauthenticatedBody::SetupQuery,
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &req).unwrap();
        let decoded: WireRequest = read_frame(&mut Cursor::new(&buf)).unwrap();
        match decoded {
            WireRequest::Unauthenticated { request_id, body } => {
                assert_eq!(request_id, 42);
                assert!(matches!(body, UnauthenticatedBody::SetupQuery));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn roundtrip_wire_response() {
        let resp = WireResponse::Ack { request_id: 7 };
        let mut buf = Vec::new();
        write_frame(&mut buf, &resp).unwrap();
        let decoded: WireResponse = read_frame(&mut Cursor::new(&buf)).unwrap();
        match decoded {
            WireResponse::Ack { request_id } => assert_eq!(request_id, 7),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn oversized_frame_rejected() {
        let len = (MAX_FRAME_SIZE + 1).to_be_bytes();
        let mut cursor = Cursor::new(len.to_vec());
        let result: io::Result<WireRequest> = read_frame(&mut cursor);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn eof_on_empty_stream() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        let result: io::Result<WireRequest> = read_frame(&mut cursor);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }
}
