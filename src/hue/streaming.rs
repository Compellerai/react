//! Hue Entertainment API DTLS streaming
//!
//! Sends binary RGB packets to the Hue bridge over a DTLS 1.2 connection
//! using PSK cipher TLS_PSK_WITH_AES_128_GCM_SHA256.
//!
//! ## Packet format (big-endian, v1 Entertainment API)
//! ```text
//! "HueStream" (9B) | 0x01 0x00 | seq:u8 | 0x00 0x00 | 0x00 (RGB) | 0x00
//! Per light (9B each): device_type:u8(0x00) | light_id:u16 | R:u16 | G:u16 | B:u16
//! ```
//!
//! ## DTLS Implementation
//! Uses OpenSSL's datagram BIO (`BIO_new_dgram`) via `openssl-sys` FFI for proper
//! DTLS 1.2 over UDP. The high-level Rust openssl crate's `SslStream` only supports
//! stream BIO (TLS/TCP), so we use the low-level API directly for DTLS.

use std::net::UdpSocket;
use std::os::unix::io::AsRawFd;
use std::time::Duration;
use openssl::ssl::{SslContext, SslMethod, SslVerifyMode, SslOptions, Ssl};
use foreign_types_shared::ForeignType;

// BIO_new_dgram isn't in openssl-sys bindings but exists in the vendored OpenSSL library.
// We declare it manually for DTLS datagram BIO support.
extern "C" {
    fn BIO_new_dgram(fd: std::os::raw::c_int, close_flag: std::os::raw::c_int) -> *mut openssl_sys::BIO;
}

/// RGB color for a Hue light (16-bit per channel, 0-65535)
#[derive(Clone, Copy, Debug)]
pub struct HueLightColor {
    /// v1 API light resource ID (sent as u16 big-endian in packet)
    pub light_id: u16,
    pub r: u16,
    pub g: u16,
    pub b: u16,
}

impl HueLightColor {
    pub fn new(light_id: u16, r: u16, g: u16, b: u16) -> Self {
        Self { light_id, r, g, b }
    }

    /// Create from floating-point RGB (0.0 - 1.0)
    pub fn from_f32(light_id: u16, r: f32, g: f32, b: f32) -> Self {
        Self {
            light_id,
            r: (r.clamp(0.0, 1.0) * 65535.0) as u16,
            g: (g.clamp(0.0, 1.0) * 65535.0) as u16,
            b: (b.clamp(0.0, 1.0) * 65535.0) as u16,
        }
    }

    /// Convert back to floating-point RGB (0.0 - 1.0) for interpolation
    pub fn as_f32(&self) -> (f32, f32, f32) {
        (
            self.r as f32 / 65535.0,
            self.g as f32 / 65535.0,
            self.b as f32 / 65535.0,
        )
    }
}

/// Build a Hue Entertainment API binary packet
///
/// Format: Header (16 bytes) + per-light data (9 bytes each)
/// Max 10 lights per entertainment area
pub fn build_entertainment_packet(sequence: u8, lights: &[HueLightColor]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(16 + lights.len() * 9);

    // Protocol name
    packet.extend_from_slice(b"HueStream");
    // API version 1.0 (matches v1 REST API activation)
    packet.push(0x01);
    packet.push(0x00);
    // Sequence number
    packet.push(sequence);
    // Reserved
    packet.push(0x00);
    packet.push(0x00);
    // Color space: 0x00 = RGB, 0x01 = XY+Brightness
    packet.push(0x00);
    // Reserved
    packet.push(0x00);

    // Per-light data: 9 bytes each (max 10 lights)
    for light in lights.iter().take(10) {
        packet.push(0x00); // Device type: 0x00 = Light
        packet.extend_from_slice(&light.light_id.to_be_bytes()); // u16 big-endian
        packet.extend_from_slice(&light.r.to_be_bytes());
        packet.extend_from_slice(&light.g.to_be_bytes());
        packet.extend_from_slice(&light.b.to_be_bytes());
    }

    packet
}

/// RGB color for a Hue v2 channel (u8 channel ID + 16-bit per channel)
#[derive(Clone, Copy, Debug)]
pub struct HueChannelColor {
    /// v2 API channel ID (u8, from entertainment_configuration channels[])
    pub channel_id: u8,
    pub r: u16,
    pub g: u16,
    pub b: u16,
}

impl HueChannelColor {
    /// Create from floating-point RGB (0.0 - 1.0)
    pub fn from_f32(channel_id: u8, r: f32, g: f32, b: f32) -> Self {
        Self {
            channel_id,
            r: (r.clamp(0.0, 1.0) * 65535.0) as u16,
            g: (g.clamp(0.0, 1.0) * 65535.0) as u16,
            b: (b.clamp(0.0, 1.0) * 65535.0) as u16,
        }
    }

    /// Convert back to floating-point RGB (0.0 - 1.0) for interpolation
    pub fn as_f32(&self) -> (f32, f32, f32) {
        (
            self.r as f32 / 65535.0,
            self.g as f32 / 65535.0,
            self.b as f32 / 65535.0,
        )
    }
}

/// Build a Hue Entertainment API v2 binary packet
///
/// Format: Header (16 bytes) + UUID (36 bytes ASCII) + per-channel data (7 bytes each)
/// Max 20 channels per entertainment area
///
/// Header: "HueStream" (9B) + version 0x02 0x00 + seq:u8 + 0x00 0x00 + 0x00 (RGB) + 0x00
/// UUID: 36 ASCII characters (with dashes, e.g. "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx")
/// Per channel (7B): channel_id:u8 + R:u16 + G:u16 + B:u16
pub fn build_entertainment_packet_v2(sequence: u8, area_uuid: &str, channels: &[HueChannelColor]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(16 + 36 + channels.len() * 7);

    // Protocol name
    packet.extend_from_slice(b"HueStream");
    // API version 2.0
    packet.push(0x02);
    packet.push(0x00);
    // Sequence number
    packet.push(sequence);
    // Reserved
    packet.push(0x00);
    packet.push(0x00);
    // Color space: 0x00 = RGB
    packet.push(0x00);
    // Reserved
    packet.push(0x00);

    // Entertainment area UUID (36 ASCII bytes, zero-padded if shorter)
    let uuid_bytes = area_uuid.as_bytes();
    for i in 0..36 {
        if i < uuid_bytes.len() {
            packet.push(uuid_bytes[i]);
        } else {
            packet.push(0x00);
        }
    }

    // Per-channel data: 7 bytes each (max 20 channels)
    for ch in channels.iter().take(20) {
        packet.push(ch.channel_id);
        packet.extend_from_slice(&ch.r.to_be_bytes());
        packet.extend_from_slice(&ch.g.to_be_bytes());
        packet.extend_from_slice(&ch.b.to_be_bytes());
    }

    packet
}

// ============================================================================
// DTLS CONNECTION (low-level openssl-sys FFI for datagram BIO)
// ============================================================================

/// DTLS connection over UDP using OpenSSL's datagram BIO.
///
/// The Rust openssl crate's `SslStream` uses stream-based memory BIO which
/// doesn't work for DTLS. This struct manages the SSL object directly via
/// `openssl-sys` FFI with a proper datagram BIO (`BIO_new_dgram`).
struct DtlsConnection {
    /// Raw pointer to the SSL object — used for SSL_write/SSL_read/SSL_shutdown
    ssl_ptr: *mut openssl_sys::SSL,
    /// Owns the SSL object; Drop calls SSL_free which frees the BIO too
    _ssl: Ssl,
    /// Keeps the UDP socket file descriptor alive (BIO uses BIO_NOCLOSE)
    _socket: UdpSocket,
    /// Keeps the SSL context alive for the connection's duration
    _ctx: SslContext,
}

// SSL* is not Send by default, but our usage is single-threaded (sender loop)
unsafe impl Send for DtlsConnection {}

impl DtlsConnection {
    fn write(&self, data: &[u8]) -> Result<usize, String> {
        unsafe {
            let ret = openssl_sys::SSL_write(
                self.ssl_ptr,
                data.as_ptr() as *const std::ffi::c_void,
                data.len() as i32,
            );
            if ret <= 0 {
                let err = openssl_sys::SSL_get_error(self.ssl_ptr, ret);
                return Err(format!("SSL_write error code {}", err));
            }
            Ok(ret as usize)
        }
    }

    fn shutdown(&self) {
        unsafe {
            openssl_sys::SSL_shutdown(self.ssl_ptr);
        }
    }
}

/// Establish DTLS 1.2 connection to Hue bridge using PSK with datagram BIO
///
/// `psk_identity` — PSK identity for DTLS handshake:
///   - v1: the `username` from push-link pairing
///   - v2: the `hue-application-id` from GET /auth/v1 response header
fn connect_dtls(bridge_ip: &str, psk_identity: &str, clientkey: &str) -> Result<DtlsConnection, String> {
    let target_addr = format!("{}:2100", bridge_ip);

    // Create and connect UDP socket
    let socket = UdpSocket::bind("0.0.0.0:0")
        .map_err(|e| format!("Bind UDP: {}", e))?;
    socket.connect(&target_addr)
        .map_err(|e| format!("Connect UDP to {}: {}", target_addr, e))?;
    socket.set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| format!("Set read timeout: {}", e))?;
    socket.set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| format!("Set write timeout: {}", e))?;

    // Decode hex clientkey to raw PSK bytes
    let psk_bytes = hex_decode(clientkey)
        .map_err(|e| format!("Invalid clientkey hex: {}", e))?;
    let identity = psk_identity.as_bytes().to_vec();

    log::info!("[Hue] PSK identity: ({} bytes), PSK: {} bytes",
        identity.len(), psk_bytes.len());

    // Build DTLS context with PSK
    let mut ctx_builder = SslContext::builder(SslMethod::dtls())
        .map_err(|e| format!("SSL context: {}", e))?;
    ctx_builder.set_verify(SslVerifyMode::NONE);
    ctx_builder.set_cipher_list("PSK-AES128-GCM-SHA256")
        .map_err(|e| format!("Set cipher: {}", e))?;

    let psk = psk_bytes;
    let ident = identity;
    ctx_builder.set_psk_client_callback(move |_ssl, _hint, identity_buf, psk_buf| {
        // Write PSK identity (null-terminated C string)
        let len = ident.len().min(identity_buf.len().saturating_sub(1));
        identity_buf[..len].copy_from_slice(&ident[..len]);
        if len < identity_buf.len() {
            identity_buf[len] = 0;
        }
        // Write PSK
        let psk_len = psk.len().min(psk_buf.len());
        psk_buf[..psk_len].copy_from_slice(&psk[..psk_len]);
        Ok(psk_len)
    });

    // Set NO_QUERY_MTU on context before building (avoids PMTU discovery issues)
    ctx_builder.set_options(SslOptions::NO_QUERY_MTU);

    let ctx = ctx_builder.build();
    let ssl = Ssl::new(&ctx).map_err(|e| format!("SSL new: {}", e))?;
    let ssl_ptr = ssl.as_ptr();

    // Set up datagram BIO on the connected UDP socket (proper DTLS I/O layer)
    unsafe {
        // BIO_new_dgram: proper datagram BIO for DTLS over UDP
        // 0 = BIO_NOCLOSE — we manage the socket lifetime via _socket field
        let bio = BIO_new_dgram(socket.as_raw_fd(), 0);
        if bio.is_null() {
            return Err("BIO_new_dgram failed — cannot create datagram BIO".to_string());
        }
        log::info!("[Hue] Datagram BIO created for fd={}", socket.as_raw_fd());
        openssl_sys::SSL_set_bio(ssl_ptr, bio, bio);

        // Set MTU via SSL_ctrl (SSL_set_mtu is a C macro, not in openssl-sys)
        // SSL_CTRL_SET_MTU = 17
        openssl_sys::SSL_ctrl(ssl_ptr, 17, 1200, std::ptr::null_mut());
    }

    log::info!("[Hue] Starting DTLS handshake with {}...", target_addr);

    // Perform DTLS handshake
    let ret = unsafe { openssl_sys::SSL_connect(ssl_ptr) };
    if ret != 1 {
        let err_code = unsafe { openssl_sys::SSL_get_error(ssl_ptr, ret) };
        // Get detailed error string from OpenSSL error queue
        let err_detail = openssl::error::ErrorStack::get().to_string();
        return Err(format!(
            "DTLS handshake failed (SSL_connect={}, err_code={}, detail={})",
            ret, err_code, err_detail
        ));
    }

    // Log cipher info
    unsafe {
        let cipher = openssl_sys::SSL_get_current_cipher(ssl_ptr);
        if !cipher.is_null() {
            let name = std::ffi::CStr::from_ptr(openssl_sys::SSL_CIPHER_get_name(cipher));
            log::info!("[Hue] DTLS connected (cipher: {})", name.to_string_lossy());
        } else {
            log::info!("[Hue] DTLS connected (cipher: unknown)");
        }
    }

    Ok(DtlsConnection {
        ssl_ptr,
        _ssl: ssl,
        _socket: socket,
        _ctx: ctx,
    })
}

/// Decode hex string to bytes
fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    if hex.len() % 2 != 0 {
        return Err("Odd-length hex string".to_string());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|e| format!("at position {}: {}", i, e))
        })
        .collect()
}

// ============================================================================
// STREAMER (direct fire-and-forget — no background thread)
// ============================================================================

/// Hue Entertainment streamer — sends packets to bridge via DTLS.
/// Direct fire-and-forget: encrypt + UDP sendto inline, no channel, no thread.
pub struct HueStreamer {
    dtls: DtlsConnection,
    packet_count: u64,
}

impl HueStreamer {
    /// Create a new streamer with DTLS connection to the bridge
    pub fn new(bridge_ip: &str, psk_identity: &str, clientkey: &str) -> Result<Self, String> {
        let dtls = connect_dtls(bridge_ip, psk_identity, clientkey)?;
        log::info!("[Hue] DTLS streamer ready (direct fire-and-forget)");
        Ok(Self { dtls, packet_count: 0 })
    }

    /// Send a pre-built entertainment packet (encrypt + UDP send, inline).
    pub fn send(&mut self, packet: &[u8]) {
        match self.dtls.write(packet) {
            Ok(_) => {
                self.packet_count += 1;
                if self.packet_count == 1 || self.packet_count % 500 == 0 {
                    log::info!("[Hue] Sent {} DTLS packets", self.packet_count);
                }
            }
            Err(e) => {
                log::warn!("[Hue] DTLS write failed: {}", e);
            }
        }
    }
}

impl Drop for HueStreamer {
    fn drop(&mut self) {
        log::info!("[Hue] DTLS streamer shutting down (sent {} packets)", self.packet_count);
        self.dtls.shutdown();
    }
}
