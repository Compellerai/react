//! Ableton Link tempo listener — clean-room.
//!
//! Passively joins the Ableton Link discovery multicast group and decodes the
//! session tempo. **Clean-room**: the protocol was reverse-engineered from
//! observed packet captures (tcpdump of Ableton Live), never from Ableton's
//! GPL source — see the Brain milestone "Ableton Link tempo field decoded".
//! No Ableton code, no third-party Link crate — all of this is ours.
//!
//! Link is a *tempo* source here: we read the session BPM and leave beat phase
//! to REACT's own audio. The listener runs on its own thread and exposes the
//! latest tempo plus when it was last heard, for the ShowClock selector.

// Whole module is wired into the ShowClock poll in the next step.
#![allow(dead_code)]

use socket2::{Domain, Protocol, Socket, Type};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Ableton Link discovery multicast group + port. 224.76.78.75 — the last
/// three octets are 'L', 'N', 'K' in ASCII.
const LINK_GROUP: Ipv4Addr = Ipv4Addr::new(224, 76, 78, 75);
const LINK_PORT: u16 = 20808;

/// ASCII magic prefixing every Ableton Link packet payload.
const LINK_MAGIC: &[u8] = b"_asdp_v";
/// TLV key of the timeline entry — the one carrying tempo.
const TMLN_KEY: &[u8] = b"tmln";

/// Latest Link tempo reading, shared between the listener thread and callers.
#[derive(Clone, Copy, Debug, Default)]
struct LinkState {
    /// Session tempo in BPM — `None` until a Link session has been seen.
    tempo_bpm: Option<f32>,
    /// When the last Link packet was received.
    last_seen: Option<Instant>,
}

/// Background Ableton Link tempo listener. Construct once; it runs until dropped.
pub struct AbletonLinkListener {
    state: Arc<Mutex<LinkState>>,
    running: Arc<AtomicBool>,
}

impl AbletonLinkListener {
    /// Start listening for Ableton Link on the LAN. Spawns a background thread
    /// and returns immediately. If the socket cannot be opened the listener is
    /// simply inert — it never reports a tempo — never fatal to REACT.
    pub fn new() -> Self {
        let state = Arc::new(Mutex::new(LinkState::default()));
        let running = Arc::new(AtomicBool::new(true));

        let thread_state = state.clone();
        let thread_running = running.clone();
        thread::Builder::new()
            .name("ableton-link-listener".to_string())
            .spawn(move || listener_loop(thread_state, thread_running))
            .ok();

        Self { state, running }
    }

    /// Latest Link tempo (BPM) paired with when it was last heard. `None` until
    /// a Link session is seen. The caller applies its own staleness window.
    pub fn tempo(&self) -> Option<(f32, Instant)> {
        let s = self.state.lock().ok()?;
        Some((s.tempo_bpm?, s.last_seen?))
    }
}

impl Default for AbletonLinkListener {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AbletonLinkListener {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

/// Local IPv4 of every usable interface — the set the Link multicast group is
/// joined on. This Mac is heavily multi-homed (Wi-Fi, Ethernet, Thunderbolt
/// bridges, 8+ VPN tunnels), so guessing one "LAN interface" is unreliable —
/// we enumerate via `getifaddrs` and join the group on every interface.
///
/// Loopback (`127.0.0.1`) is deliberately INCLUDED: Ableton sends Link
/// discovery over `lo0` for same-machine peers, so when Ableton and REACT run
/// on one Mac, loopback is the interface the traffic actually arrives on.
fn multicast_interfaces() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    // SAFETY: textbook getifaddrs walk — the list is freed before return, and
    // every pointer is null-checked before it is dereferenced.
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return out;
        }
        let mut cur = ifap;
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.ifa_addr.is_null()
                && i32::from((*ifa.ifa_addr).sa_family) == libc::AF_INET
            {
                let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                if !ip.is_unspecified() && !ip.is_link_local() {
                    out.push(ip);
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
    }
    out
}

/// Open the multicast receive socket, joined to the Link group on every usable
/// interface. `SO_REUSEADDR` + `SO_REUSEPORT` let us co-bind UDP 20808 while
/// Ableton (or another Link app) already holds it on the same machine — macOS
/// needs `SO_REUSEPORT` for a shared bind; `SO_REUSEADDR` alone fails.
fn open_socket() -> std::io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, LINK_PORT)).into())?;
    let socket: UdpSocket = socket.into();

    // Join on every interface — the one that carries Link traffic is among them.
    let mut joined = 0usize;
    for iface in multicast_interfaces() {
        match socket.join_multicast_v4(&LINK_GROUP, &iface) {
            Ok(()) => {
                joined += 1;
                log::info!("[AbletonLink] joined group {LINK_GROUP} on {iface}");
            }
            Err(e) => log::debug!("[AbletonLink] could not join on {iface}: {e}"),
        }
    }
    if joined == 0 {
        // Nothing enumerated — fall back to the kernel's default interface.
        socket.join_multicast_v4(&LINK_GROUP, &Ipv4Addr::UNSPECIFIED)?;
        log::warn!("[AbletonLink] no interfaces enumerated — joined on default");
    }

    // Wake periodically so the loop can observe the shutdown flag.
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
    Ok(socket)
}

/// Listener thread body: receive Link packets, decode tempo into shared state.
fn listener_loop(state: Arc<Mutex<LinkState>>, running: Arc<AtomicBool>) {
    let socket = match open_socket() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[AbletonLink] listener disabled: {e}");
            return;
        }
    };
    log::info!("[AbletonLink] listening on {LINK_GROUP}:{LINK_PORT}");

    let mut buf = [0u8; 2048];
    // Heartbeat so a successful decode is visible in the log.
    let mut last_log = Instant::now() - Duration::from_secs(10);
    // Diagnostic counters — every 5s we report raw vs decoded packet counts, so a
    // deaf socket (0 raw) is distinguishable from a decode failure (raw, 0 decoded).
    let mut raw = 0u64;
    let mut decoded = 0u64;
    let mut last_stat = Instant::now();
    while running.load(Ordering::Relaxed) {
        match socket.recv_from(&mut buf) {
            Ok((len, src)) => {
                raw += 1;
                match parse_tempo(&buf[..len]) {
                    Some(bpm) => {
                        decoded += 1;
                        if let Ok(mut s) = state.lock() {
                            s.tempo_bpm = Some(bpm);
                            s.last_seen = Some(Instant::now());
                        }
                        if last_log.elapsed() >= Duration::from_secs(2) {
                            log::info!("[AbletonLink] receiving — tempo {bpm:.2} BPM");
                            last_log = Instant::now();
                        }
                    }
                    None => log::debug!("[AbletonLink] undecoded {len}-byte packet from {src}"),
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => {
                log::warn!("[AbletonLink] recv error: {e}");
                return;
            }
        }
        if last_stat.elapsed() >= Duration::from_secs(5) {
            log::info!("[AbletonLink] recv stats — {raw} raw, {decoded} decoded (last 5s)");
            raw = 0;
            decoded = 0;
            last_stat = Instant::now();
        }
    }
}

/// Decode the session tempo (BPM) from a raw Ableton Link UDP payload.
///
/// Clean-room, from observed packets: the `tmln` timeline entry's first 8 bytes
/// are a big-endian microseconds-per-beat count; `BPM = 60_000_000 / value`.
fn parse_tempo(payload: &[u8]) -> Option<f32> {
    if !payload.starts_with(LINK_MAGIC) {
        return None;
    }
    let key_at = payload.windows(TMLN_KEY.len()).position(|w| w == TMLN_KEY)?;
    let value_at = key_at + TMLN_KEY.len() + 4; // skip the 4-char key + 4-byte length
    let tempo_bytes: [u8; 8] = payload.get(value_at..value_at + 8)?.try_into().ok()?;
    let micros_per_beat = u64::from_be_bytes(tempo_bytes);
    if micros_per_beat == 0 {
        return None;
    }
    Some((60_000_000.0 / micros_per_beat as f64) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 107-byte Link payload captured from Ableton Live at 120.00 BPM (tcpdump,
    /// 2026-05-16) — the clean-room reference + decode regression fixture.
    const CAPTURE_120_BPM: [u8; 107] = [
        0x5f, 0x61, 0x73, 0x64, 0x70, 0x5f, 0x76, 0x01, 0x01, 0x05, 0x00, 0x00,
        0x37, 0x50, 0x71, 0x39, 0x5b, 0x52, 0x68, 0x74,
        0x74, 0x6d, 0x6c, 0x6e, 0x00, 0x00, 0x00, 0x18,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x07, 0xa1, 0x20,
        0x00, 0x00, 0x00, 0x00, 0x52, 0xa7, 0xfc, 0x3f,
        0x00, 0x00, 0x00, 0x00, 0x29, 0x62, 0x0d, 0x4a,
        0x73, 0x65, 0x73, 0x73, 0x00, 0x00, 0x00, 0x08,
        0x37, 0x50, 0x71, 0x39, 0x5b, 0x52, 0x68, 0x74,
        0x73, 0x74, 0x73, 0x74, 0x00, 0x00, 0x00, 0x11,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
        0x6d, 0x65, 0x70, 0x34, 0x00, 0x00, 0x00, 0x06,
        0x7f, 0x00, 0x00, 0x01, 0xfa, 0x7a,
    ];

    #[test]
    fn decodes_120_bpm_from_real_capture() {
        let bpm = parse_tempo(&CAPTURE_120_BPM).expect("tmln entry should be present");
        assert!((bpm - 120.0).abs() < 0.001, "expected 120.0 BPM, got {bpm}");
    }

    #[test]
    fn rejects_non_link_payload() {
        assert!(parse_tempo(b"not an ableton link packet").is_none());
    }

    #[test]
    fn rejects_payload_truncated_before_timeline() {
        assert!(parse_tempo(&CAPTURE_120_BPM[..20]).is_none());
    }
}
