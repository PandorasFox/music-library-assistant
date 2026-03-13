//! Wire transport types and framing for socket-based protocol communication.
//!
//! Provides:
//! - `WireRequest` / `WireResponse` — envelope types mapping 1:1 to `HandleCommand`
//!   minus lifecycle variants (Shutdown) and minus in-process reply channels.
//! - Length-prefixed bincode framing for `std::io::Read` / `Write` streams.
//!
//! The framing is synchronous (blocking I/O), matching the plain
//! `std::thread` + `std::net::UnixStream` transport design.

use std::io::{self, Read, Write};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::auth::SessionToken;
use crate::protocol::{
    AuthenticatedBody, AuthenticatedResponse, ProtocolError, UnauthenticatedBody,
    UnauthenticatedResponse,
};

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
        token: SessionToken,
        body: Box<AuthenticatedBody>,
    },
    /// Unauthenticated protocol request (login, setup query).
    Unauthenticated(UnauthenticatedBody),
    /// Notify server that DB is ready (post-setup lifecycle signal).
    NotifyDbReady,
}

/// Server → Client response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireResponse {
    /// Response to an authenticated request.
    Authenticated(Box<Result<AuthenticatedResponse, ProtocolError>>),
    /// Response to an unauthenticated request.
    Unauthenticated(Result<UnauthenticatedResponse, ProtocolError>),
    /// Acknowledgement for lifecycle signals (NotifyDbReady).
    Ack,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip_wire_request() {
        let req = WireRequest::Unauthenticated(UnauthenticatedBody::SetupQuery);
        let mut buf = Vec::new();
        write_frame(&mut buf, &req).unwrap();
        let decoded: WireRequest = read_frame(&mut Cursor::new(&buf)).unwrap();
        assert!(matches!(
            decoded,
            WireRequest::Unauthenticated(UnauthenticatedBody::SetupQuery)
        ));
    }

    #[test]
    fn roundtrip_wire_response() {
        let resp = WireResponse::Ack;
        let mut buf = Vec::new();
        write_frame(&mut buf, &resp).unwrap();
        let decoded: WireResponse = read_frame(&mut Cursor::new(&buf)).unwrap();
        assert!(matches!(decoded, WireResponse::Ack));
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
