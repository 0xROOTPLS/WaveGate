//! WebSocket framing module.
//!
//! Provides WebSocket handshake and frame encoding/decoding to make
//! traffic look like standard web traffic to firewalls.

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
            _ => Opcode::Binary, // Default to binary for unknown
        }
    }
}

/// Perform WebSocket client handshake
pub async fn client_handshake<S>(
    stream: &mut S,
    host: &str,
    path: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // Generate random 16-byte key and base64 encode it
    let mut key_bytes = [0u8; 16];
    for i in 0..16 {
        key_bytes[i] = fastrand::u8(..);
    }
    let key = base64::engine::general_purpose::STANDARD.encode(key_bytes);

    // Build HTTP upgrade request
    let request = format!(
        "GET {} HTTP/1.1\r\n\
         Host: {}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {}\r\n\
         Sec-WebSocket-Version: 13\r\n\
         User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36\r\n\
         \r\n",
        path, host, key
    );

    // Send request
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;

    // Read response
    let mut response = Vec::with_capacity(1024);
    let mut buf = [0u8; 1];

    // Read until we see \r\n\r\n
    loop {
        stream.read_exact(&mut buf).await?;
        response.push(buf[0]);

        if response.len() >= 4 {
            let len = response.len();
            if &response[len-4..] == b"\r\n\r\n" {
                break;
            }
        }

        if response.len() > 4096 {
            return Err("HTTP response too large".into());
        }
    }

    let response_str = String::from_utf8_lossy(&response);

    // Verify 101 Switching Protocols
    if !response_str.starts_with("HTTP/1.1 101") {
        return Err(format!("WebSocket upgrade failed: {}",
            response_str.lines().next().unwrap_or("unknown")).into());
    }

    // Verify Sec-WebSocket-Accept
    let expected_accept = compute_accept_key(&key);

    let has_valid_accept = response_str
        .lines()
        .any(|line| {
            if let Some(value) = line.strip_prefix("Sec-WebSocket-Accept:") {
                value.trim() == expected_accept
            } else {
                false
            }
        });

    if !has_valid_accept {
        return Err("Invalid Sec-WebSocket-Accept".into());
    }

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

/// Write a WebSocket frame (client-side, masked)
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

    // Generate masking key
    let mask: [u8; 4] = [
        fastrand::u8(..),
        fastrand::u8(..),
        fastrand::u8(..),
        fastrand::u8(..),
    ];

    // Build header
    let mut header = Vec::with_capacity(14);
    header.push(first_byte);

    // Second byte: MASK + payload length
    if len < 126 {
        header.push(0x80 | len as u8); // MASK bit set
    } else if len < 65536 {
        header.push(0x80 | 126);
        header.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        header.push(0x80 | 127);
        header.extend_from_slice(&(len as u64).to_be_bytes());
    }

    // Add masking key
    header.extend_from_slice(&mask);

    // Write header
    writer.write_all(&header).await?;

    // Write masked payload
    let mut masked = Vec::with_capacity(len);
    for (i, byte) in payload.iter().enumerate() {
        masked.push(byte ^ mask[i % 4]);
    }
    writer.write_all(&masked).await?;

    Ok(())
}

/// Read a WebSocket frame (server-side frames are not masked)
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

    // Read masking key if present
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

/// WebSocket writer wrapper that frames all writes
pub struct WsWriter<W> {
    inner: W,
}

impl<W: AsyncWrite + Unpin> WsWriter<W> {
    pub fn new(inner: W) -> Self {
        Self { inner }
    }

    pub async fn write_message(&mut self, data: &[u8]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        write_frame(&mut self.inner, Opcode::Binary, data).await
    }

    pub async fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush().await
    }
}

/// WebSocket reader wrapper that deframes all reads
pub struct WsReader<R> {
    inner: R,
}

impl<R: AsyncRead + Unpin> WsReader<R> {
    pub fn new(inner: R) -> Self {
        Self { inner }
    }

    pub async fn read_message(&mut self) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        loop {
            let (opcode, payload) = read_frame(&mut self.inner).await?;

            match opcode {
                Opcode::Binary | Opcode::Text => return Ok(payload),
                Opcode::Ping => {
                    // For ping, we'd need to send pong, but we don't have writer access here
                    // In practice, server shouldn't send pings in our use case
                    continue;
                }
                Opcode::Close => {
                    return Err("WebSocket closed".into());
                }
                _ => continue,
            }
        }
    }
}
