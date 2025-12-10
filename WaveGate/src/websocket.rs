//! WebSocket server-side handling.
//!
//! Detects WebSocket upgrade requests and handles framing for
//! connections using WebSocket mode.

use base64::Engine;
use sha1::{Sha1, Digest};
use tokio::io::{AsyncRead, AsyncWrite, AsyncReadExt, AsyncWriteExt};

/// WebSocket GUID used in handshake
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// WebSocket opcodes
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum Opcode {
    Continuation = 0x0,
    Text = 0x1,
    Binary = 0x2,
    Close = 0x8,
    Ping = 0x9,
    Pong = 0xA,
}

impl From<u8> for Opcode {
    fn from(value: u8) -> Self {
        match value & 0x0F {
            0x0 => Opcode::Continuation,
            0x1 => Opcode::Text,
            0x2 => Opcode::Binary,
            0x8 => Opcode::Close,
            0x9 => Opcode::Ping,
            0xA => Opcode::Pong,
            _ => Opcode::Binary,
        }
    }
}

/// Result of trying to detect WebSocket upgrade
pub enum UpgradeResult {
    /// This is a WebSocket upgrade request with the given key
    WebSocket(String),
    /// This is not a WebSocket request; contains the first bytes read
    NotWebSocket(Vec<u8>),
}

/// Try to detect if incoming connection is a WebSocket upgrade request.
/// Returns the WebSocket key if it is, or the bytes read if not.
pub async fn detect_websocket_upgrade<S>(
    stream: &mut S,
) -> Result<UpgradeResult, Box<dyn std::error::Error + Send + Sync>>
where
    S: AsyncRead + Unpin,
{
    // Read first few bytes to detect HTTP
    let mut peek_buf = [0u8; 4];
    stream.read_exact(&mut peek_buf).await?;

    // Check if it looks like an HTTP request (GET )
    if &peek_buf != b"GET " {
        return Ok(UpgradeResult::NotWebSocket(peek_buf.to_vec()));
    }

    // Read rest of HTTP request
    let mut request = Vec::from(peek_buf.as_slice());
    let mut buf = [0u8; 1];

    loop {
        stream.read_exact(&mut buf).await?;
        request.push(buf[0]);

        if request.len() >= 4 {
            let len = request.len();
            if &request[len-4..] == b"\r\n\r\n" {
                break;
            }
        }

        if request.len() > 4096 {
            return Err("HTTP request too large".into());
        }
    }

    let request_str = String::from_utf8_lossy(&request);

    // Check for WebSocket upgrade headers
    let is_upgrade = request_str.to_lowercase().contains("upgrade: websocket");
    let is_connection_upgrade = request_str.to_lowercase().contains("connection: upgrade");

    if !is_upgrade || !is_connection_upgrade {
        return Ok(UpgradeResult::NotWebSocket(request));
    }

    // Extract Sec-WebSocket-Key
    let key = request_str
        .lines()
        .find(|line| line.to_lowercase().starts_with("sec-websocket-key:"))
        .and_then(|line| line.split(':').nth(1))
        .map(|k| k.trim().to_string())
        .ok_or("Missing Sec-WebSocket-Key")?;

    Ok(UpgradeResult::WebSocket(key))
}

/// Complete the WebSocket handshake by sending the upgrade response.
pub async fn accept_websocket<S>(
    stream: &mut S,
    client_key: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    S: AsyncWrite + Unpin,
{
    // Compute accept key
    let accept_key = compute_accept_key(client_key);

    // Build response
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\
         \r\n",
        accept_key
    );

    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;

    Ok(())
}

/// Compute the Sec-WebSocket-Accept value
fn compute_accept_key(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WS_GUID.as_bytes());
    let result = hasher.finalize();
    base64::engine::general_purpose::STANDARD.encode(result)
}

/// Write a WebSocket frame (server-side, NOT masked)
pub async fn write_frame<W>(
    writer: &mut W,
    opcode: Opcode,
    payload: &[u8],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    W: AsyncWrite + Unpin,
{
    let len = payload.len();

    // First byte: FIN + opcode
    let first_byte = 0x80 | (opcode as u8); // FIN = 1

    // Build header (server frames are NOT masked)
    let mut header = Vec::with_capacity(10);
    header.push(first_byte);

    // Second byte: payload length (no mask bit)
    if len < 126 {
        header.push(len as u8);
    } else if len < 65536 {
        header.push(126);
        header.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        header.push(127);
        header.extend_from_slice(&(len as u64).to_be_bytes());
    }

    // Write header and payload
    writer.write_all(&header).await?;
    writer.write_all(payload).await?;

    Ok(())
}

/// Read a WebSocket frame (client frames ARE masked)
pub async fn read_frame<R>(
    reader: &mut R,
) -> Result<(Opcode, Vec<u8>), Box<dyn std::error::Error + Send + Sync>>
where
    R: AsyncRead + Unpin,
{
    // Read first two bytes
    let mut header = [0u8; 2];
    reader.read_exact(&mut header).await?;

    let _fin = (header[0] & 0x80) != 0;
    let opcode = Opcode::from(header[0]);
    let masked = (header[1] & 0x80) != 0;
    let mut len = (header[1] & 0x7F) as u64;

    // Extended payload length
    if len == 126 {
        let mut ext = [0u8; 2];
        reader.read_exact(&mut ext).await?;
        len = u16::from_be_bytes(ext) as u64;
    } else if len == 127 {
        let mut ext = [0u8; 8];
        reader.read_exact(&mut ext).await?;
        len = u64::from_be_bytes(ext);
    }

    // Sanity check
    if len > 100 * 1024 * 1024 {
        return Err("Frame too large".into());
    }

    // Read masking key if present (should be for client -> server)
    let mask = if masked {
        let mut m = [0u8; 4];
        reader.read_exact(&mut m).await?;
        Some(m)
    } else {
        None
    };

    // Read payload
    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload).await?;

    // Unmask if needed
    if let Some(mask) = mask {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[i % 4];
        }
    }

    Ok((opcode, payload))
}
