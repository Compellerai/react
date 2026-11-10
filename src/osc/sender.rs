//! OSC UDP Sender Thread
//!
//! Thread-based UDP sender for OSC messages. Uses crossbeam channel
//! for lock-free message passing from the main thread.
//!
//! ## Design
//!
//! Follows the same pattern as Pro DJ Link and MIDI:
//! - Dedicated sender thread with crossbeam channel
//! - Non-blocking send from main thread
//! - Clean shutdown via command channel

use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::thread::{self, JoinHandle};
use crossbeam_channel::{bounded, Sender, TrySendError};
use rosc::{OscMessage, OscPacket, OscType};

/// Commands sent to the sender thread
enum OscCommand {
    /// Send an OSC message
    Send {
        address: String,
        args: Vec<OscType>,
    },
    /// Update target address
    SetTarget {
        ip: String,
        port: u16,
    },
    /// Shut down the sender thread
    Shutdown,
}

/// OSC UDP Sender - manages a dedicated thread for sending OSC messages
pub struct OscSender {
    /// Command channel sender
    tx: Sender<OscCommand>,
    /// Sender thread handle
    thread: Option<JoinHandle<()>>,
}

impl OscSender {
    /// Create a new OscSender with the given target
    pub fn new(target_ip: &str, target_port: u16) -> Result<Self, String> {
        // Create command channel (bounded to prevent memory buildup)
        let (tx, rx) = bounded::<OscCommand>(256);

        // Clone strings for the thread (DNS resolution happens in thread to avoid blocking)
        let ip = target_ip.to_string();
        let port = target_port;

        // Spawn sender thread
        let thread = thread::Builder::new()
            .name("osc-sender".to_string())
            .spawn(move || {
                Self::sender_loop(rx, &ip, port);
            })
            .map_err(|e| format!("Failed to spawn sender thread: {}", e))?;

        Ok(Self {
            tx,
            thread: Some(thread),
        })
    }

    /// Send an OSC message (non-blocking)
    pub fn send(&self, address: &str, args: Vec<OscType>) {
        let cmd = OscCommand::Send {
            address: address.to_string(),
            args,
        };

        // Try to send, drop message if channel is full (don't block render)
        if let Err(TrySendError::Full(_)) = self.tx.try_send(cmd) {
            log::warn!("[OSC] Send buffer full, dropping message: {}", address);
        }
    }

    /// Update the target address
    pub fn set_target(&self, ip: &str, port: u16) {
        let cmd = OscCommand::SetTarget {
            ip: ip.to_string(),
            port,
        };
        let _ = self.tx.try_send(cmd);
    }

    /// Shut down the sender thread
    pub fn shutdown(mut self) {
        let _ = self.tx.send(OscCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// Resolve hostname to SocketAddr (supports both IP addresses and hostnames like .local).
    /// Prefers IPv4 over IPv6 — link-local IPv6 (fe80::) addresses often fail for HTTP APIs.
    fn resolve_target(host: &str, port: u16) -> Option<SocketAddr> {
        let addr_str = format!("{}:{}", host, port);
        match addr_str.to_socket_addrs() {
            Ok(addrs) => {
                let all: Vec<SocketAddr> = addrs.collect();
                if all.is_empty() {
                    log::error!("[OSC] No addresses found for {}", host);
                    return None;
                }
                // Prefer IPv4 if available
                let chosen = all.iter()
                    .find(|a| a.is_ipv4())
                    .or_else(|| all.first())
                    .copied();
                if let Some(addr) = chosen {
                    log::info!("[OSC] Resolved {} -> {} (from {} candidates)", host, addr.ip(), all.len());
                    Some(addr)
                } else {
                    None
                }
            }
            Err(e) => {
                log::error!("[OSC] Failed to resolve {}: {}", addr_str, e);
                None
            }
        }
    }

    /// Public utility to resolve a hostname to IP (for UI display).
    /// Prefers IPv4 over IPv6.
    pub fn resolve_hostname(host: &str) -> Option<String> {
        use std::net::ToSocketAddrs;
        let addr_str = format!("{}:0", host);
        match addr_str.to_socket_addrs() {
            Ok(addrs) => {
                let all: Vec<std::net::SocketAddr> = addrs.collect();
                all.iter()
                    .find(|a| a.is_ipv4())
                    .or_else(|| all.first())
                    .map(|a| a.ip().to_string())
            }
            Err(_) => None,
        }
    }

    /// Main sender loop
    fn sender_loop(rx: crossbeam_channel::Receiver<OscCommand>, initial_host: &str, initial_port: u16) {
        // Resolve initial target (DNS resolution happens here, in the thread)
        let mut target = match Self::resolve_target(initial_host, initial_port) {
            Some(addr) => addr,
            None => {
                log::error!("[OSC] Cannot start sender - failed to resolve {}:{}", initial_host, initial_port);
                return;
            }
        };

        // Bind to any available port
        let socket = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => {
                // Enable SO_BROADCAST so 255.255.255.255 and subnet broadcasts work
                let _ = s.set_broadcast(true);
                s
            }
            Err(e) => {
                log::error!("[OSC] Failed to bind UDP socket: {}", e);
                return;
            }
        };

        // UDP send is non-blocking by default, but set socket to non-blocking for consistency
        let _ = socket.set_nonblocking(true);

        log::info!("[OSC] Sender thread started, target: {}", target);

        loop {
            // Wait for commands (blocking with timeout for clean shutdown check)
            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(cmd) => match cmd {
                    OscCommand::Send { address, args } => {
                        Self::send_message(&socket, &target, &address, args);
                    }
                    OscCommand::SetTarget { ip, port } => {
                        match Self::resolve_target(&ip, port) {
                            Some(addr) => {
                                target = addr;
                                log::info!("[OSC] Target updated to {}", target);
                            }
                            None => {
                                log::error!("[OSC] Failed to resolve target {}:{}", ip, port);
                            }
                        }
                    }
                    OscCommand::Shutdown => {
                        log::info!("[OSC] Sender thread shutting down");
                        break;
                    }
                },
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    // Normal timeout, continue loop
                    continue;
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    // Channel closed, exit
                    log::info!("[OSC] Channel disconnected, sender thread exiting");
                    break;
                }
            }
        }
    }

    /// Encode and send a single OSC message
    fn send_message(socket: &UdpSocket, target: &SocketAddr, address: &str, args: Vec<OscType>) {
        let msg = OscMessage {
            addr: address.to_string(),
            args,
        };

        let packet = OscPacket::Message(msg);

        match rosc::encoder::encode(&packet) {
            Ok(buf) => {
                match socket.send_to(&buf, target) {
                    Ok(_) => {
                        // Log cue sends (not continuous values to avoid spam)
                        if address.contains("/cue") || address.contains("/trigger") {
                            log::info!("[OSC] Sent {} to {}", address, target);
                        }
                    }
                    Err(e) => {
                        log::warn!("[OSC] Send error to {}: {}", target, e);
                    }
                }
            }
            Err(e) => {
                log::error!("[OSC] Failed to encode message {}: {:?}", address, e);
            }
        }
    }

    /// Detect the subnet broadcast address for the active network interface.
    /// Returns e.g. "192.168.1.255" by connecting a UDP socket to find
    /// the local IP, then replacing the last octet with 255 (assumes /24).
    pub fn detect_broadcast_address() -> String {
        let fallback = "255.255.255.255".to_string();
        let socket = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(_) => return fallback,
        };
        // Connect to a routable address to determine which interface is active
        if socket.connect("10.255.255.255:1").is_err() {
            return fallback;
        }
        match socket.local_addr() {
            Ok(addr) => {
                if let IpAddr::V4(ip) = addr.ip() {
                    let octets = ip.octets();
                    let broadcast = Ipv4Addr::new(octets[0], octets[1], octets[2], 255);
                    broadcast.to_string()
                } else {
                    fallback
                }
            }
            Err(_) => fallback,
        }
    }
}

impl Drop for OscSender {
    fn drop(&mut self) {
        // Signal shutdown if thread still running
        let _ = self.tx.try_send(OscCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
