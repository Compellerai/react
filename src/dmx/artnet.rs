use std::io;
use std::net::{SocketAddr, UdpSocket};

/// UDP sender for Art-Net ArtDmx packets.
pub struct ArtNetSender {
    socket: UdpSocket,
    target: SocketAddr,
    sequence: u8,
}

impl ArtNetSender {
    pub fn new(target: SocketAddr) -> io::Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_nonblocking(true)?;
        Ok(Self { socket, target, sequence: 1 })
    }

    /// The configured Art-Net target socket (for the operator status line).
    pub fn target(&self) -> SocketAddr {
        self.target
    }

    pub fn send_frame(&mut self, universe: u16, data: &[u8]) -> io::Result<usize> {
        let packet = build_artdmx_packet(universe, self.sequence, data);
        self.sequence = self.sequence.wrapping_add(1).max(1);
        self.socket.send_to(&packet, self.target)
    }
}

/// Build an Art-Net ArtDmx packet.
///
/// Art-Net uses little-endian OpCode/universe and big-endian protocol version
/// and DMX length. REACT DmxFrame emits full 512-byte payloads for predictable fixture state.
pub fn build_artdmx_packet(universe: u16, sequence: u8, data: &[u8]) -> Vec<u8> {
    let payload_len = data.len().min(512);
    let mut packet = Vec::with_capacity(18 + payload_len);
    packet.extend_from_slice(b"Art-Net\0");
    packet.extend_from_slice(&0x5000u16.to_le_bytes()); // OpOutput / ArtDmx
    packet.extend_from_slice(&14u16.to_be_bytes()); // protocol version
    packet.push(sequence);
    packet.push(0); // physical input port, unused for output
    packet.extend_from_slice(&universe.to_le_bytes());
    packet.extend_from_slice(&(payload_len as u16).to_be_bytes());
    packet.extend_from_slice(&data[..payload_len]);
    packet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artdmx_packet_header_is_valid() {
        let mut data = [0u8; 512];
        data[0] = 255;
        let packet = build_artdmx_packet(7, 9, &data);
        assert_eq!(&packet[0..8], b"Art-Net\0");
        assert_eq!(&packet[8..10], &[0x00, 0x50]);
        assert_eq!(&packet[10..12], &[0x00, 0x0e]);
        assert_eq!(packet[12], 9);
        assert_eq!(&packet[14..16], &[7, 0]);
        assert_eq!(&packet[16..18], &[2, 0]);
        assert_eq!(packet.len(), 530);
        assert_eq!(packet[18], 255);
    }
}
