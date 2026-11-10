//! WLED UDP streaming (DRGB protocol)
//!
//! Simple UDP packet sender for WLED LED strips using the DRGB protocol.
//! No auth, no DTLS — just raw UDP packets with RGB data per LED.
//!
//! ## DRGB Packet format
//! ```text
//! Byte 0: 0x02 (DRGB mode)
//! Byte 1: timeout in seconds
//! Then: R,G,B per LED (3 bytes each)
//! ```

use crossbeam_channel::{Sender, Receiver};
use std::net::UdpSocket;

/// Build a DRGB packet for WLED
///
/// - `timeout`: seconds before WLED reverts to normal mode (0 = no timeout)
/// - `leds`: RGB values per LED
pub fn build_drgb_packet(timeout: u8, leds: &[(u8, u8, u8)]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(2 + leds.len() * 3);
    packet.push(0x02); // DRGB mode
    packet.push(timeout);
    for &(r, g, b) in leds {
        packet.push(r);
        packet.push(g);
        packet.push(b);
    }
    packet
}

enum StreamCommand {
    Send(Vec<u8>),
    Shutdown,
}

/// WLED UDP streamer — sends DRGB packets via a background thread
pub struct WledStreamer {
    tx: Sender<StreamCommand>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WledStreamer {
    /// Create a new streamer connected to the given WLED device
    pub fn new(ip: &str, port: u16) -> Result<Self, String> {
        let (tx, rx) = crossbeam_channel::bounded::<StreamCommand>(4);

        let socket = UdpSocket::bind("0.0.0.0:0")
            .map_err(|e| format!("Bind UDP: {}", e))?;
        let target = format!("{}:{}", ip, port);
        socket.connect(&target)
            .map_err(|e| format!("Connect UDP to {}: {}", target, e))?;

        let ip_owned = ip.to_string();
        let thread = std::thread::Builder::new()
            .name(format!("wled-{}", ip))
            .spawn(move || {
                Self::sender_loop(rx, socket, &ip_owned);
            })
            .map_err(|e| format!("Spawn WLED streamer: {}", e))?;

        Ok(Self {
            tx,
            thread: Some(thread),
        })
    }

    /// Send a pre-built DRGB packet (non-blocking, drops if channel full)
    pub fn send(&self, packet: Vec<u8>) {
        let _ = self.tx.try_send(StreamCommand::Send(packet));
    }

    /// Shutdown the streamer gracefully
    pub fn shutdown(mut self) {
        let _ = self.tx.send(StreamCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            crate::thread_util::join_or_abandon(thread, crate::thread_util::SHUTDOWN_GRACE);
        }
    }

    fn sender_loop(rx: Receiver<StreamCommand>, socket: UdpSocket, ip: &str) {
        log::info!("[WLED] Streamer thread started for {}", ip);
        let mut packet_count: u64 = 0;

        loop {
            match rx.recv() {
                Ok(StreamCommand::Send(packet)) => {
                    match socket.send(&packet) {
                        Ok(_) => {
                            packet_count += 1;
                            if packet_count == 1 || packet_count % 500 == 0 {
                                log::info!("[WLED] {} — sent {} UDP packets", ip, packet_count);
                            }
                        }
                        Err(e) => {
                            log::warn!("[WLED] {} — UDP send failed: {}", ip, e);
                        }
                    }
                }
                Ok(StreamCommand::Shutdown) | Err(_) => {
                    log::info!("[WLED] {} — streamer shutting down (sent {} packets)", ip, packet_count);
                    break;
                }
            }
        }
    }
}

impl Drop for WledStreamer {
    fn drop(&mut self) {
        let _ = self.tx.send(StreamCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            crate::thread_util::join_or_abandon(thread, crate::thread_util::SHUTDOWN_GRACE);
        }
    }
}
